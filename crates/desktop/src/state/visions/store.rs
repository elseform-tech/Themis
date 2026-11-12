//! Reading, validating and writing the Visions store files.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use themis_core::visions::Decision;

use super::{
    Channel, Owner, Vision, VisionsCache, CHANNELS_FILE, CHANNEL_LOGS_DIR, OWNERS_FILE,
    SOUL_MAX_BYTES, VISIONS_FILE,
};
use crate::state::chat_grants::safe_chat_id;

/// Store files are read only up to this size.
const MAX_STORE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 60;
const MAX_ROLE_CHARS: usize = 200;
const MAX_LOOK_CHARS: usize = 40;
const MAX_MODEL_CHARS: usize = 200;
const MAX_TOPIC_CHARS: usize = 500;
const MAX_CHANNEL_NAME: usize = 40;
const MAX_MEMBERS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// A file exists but cannot be read; it is left as it is.
    Unreadable,
    WriteFailed,
    /// The save itself is not allowed (validation, a stale revision, an unknown id).
    Refused,
}

/// Why a store call failed: the message for the caller, and metadata for diagnostics.
#[derive(Debug)]
pub(super) struct StoreError {
    pub message: String,
    failure: Failure,
    file: &'static str,
}

impl StoreError {
    pub(super) fn refused(file: &'static str, message: &str) -> Self {
        Self {
            message: message.to_owned(),
            failure: Failure::Refused,
            file,
        }
    }

    pub(super) fn unreadable(file: &'static str, detail: &str) -> Self {
        Self {
            message: format!(
                "visions/{file} could not be read ({detail}); it was left unchanged, so fix or remove it"
            ),
            failure: Failure::Unreadable,
            file,
        }
    }

    pub(super) fn write_failed(file: &'static str, error: &std::io::Error) -> Self {
        Self {
            message: format!("visions/{file} could not be saved ({})", error.kind()),
            failure: Failure::WriteFailed,
            file,
        }
    }

    pub(super) fn event(&self) -> &'static str {
        match self.failure {
            Failure::Unreadable => "store_unreadable",
            Failure::WriteFailed => "store_write_failed",
            Failure::Refused => "save_refused",
        }
    }

    pub(super) fn file(&self) -> &'static str {
        self.file
    }

    /// A fixed reason: refusals use fixed messages, the others hold paths or details.
    pub(super) fn reason(&self) -> &str {
        match self.failure {
            Failure::Unreadable => "unreadable store file",
            Failure::WriteFailed => "store write failed",
            Failure::Refused => &self.message,
        }
    }
}

/// Fails when `path` is a symlink; a missing path is fine.
pub(super) fn refuse_symlink(path: &Path, file: &'static str) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(StoreError::unreadable(file, "it is a symbolic link"))
        }
        _ => Ok(()),
    }
}

/// Reads `dir/file`; a missing file is empty.
pub(super) fn read_store<T: DeserializeOwned + Default>(
    dir: &Path,
    file: &'static str,
) -> Result<T, StoreError> {
    refuse_symlink(dir, file)?;
    let path = dir.join(file);
    refuse_symlink(&path, file)?;
    let mut opened = match fs::File::open(&path) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(StoreError::unreadable(file, &error.kind().to_string())),
    };
    let mut bytes = Vec::new();
    (&mut opened)
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::unreadable(file, &error.kind().to_string()))?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(StoreError::unreadable(file, "it is over 32 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        let at = format!(
            "not valid at line {}, column {}",
            error.line(),
            error.column()
        );
        StoreError::unreadable(file, &at)
    })
}

/// Replaces `dir/file` with `value`: a fresh 0600 temporary file, fsync, then rename.
pub(super) fn write_store(
    dir: &Path,
    file: &'static str,
    value: &impl Serialize,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| StoreError::refused(file, "The store could not be encoded"))?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(StoreError::refused(
            file,
            "The store would pass 32 MiB; nothing was saved",
        ));
    }
    refuse_symlink(dir, file)?;
    fs::create_dir_all(dir).map_err(|error| StoreError::write_failed(file, &error))?;
    let path = dir.join(file);
    refuse_symlink(&path, file)?;
    let temp = dir.join(format!("{file}.{}.tmp", next_revision(None)));
    let written = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        fs::rename(&temp, &path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(StoreError::write_failed(file, &error));
    }
    Ok(())
}

/// A revision later than `previous`: the time in nanoseconds, as hex.
fn next_revision(previous: Option<&str>) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let after = previous
        .and_then(|previous| u128::from_str_radix(previous, 16).ok())
        .map_or(0, |previous| previous + 1);
    format!("{:x}", now.max(after))
}

/// A store file's size and modified time; None when it does not exist.
pub(super) type Stamp = Option<(u64, Option<std::time::SystemTime>)>;

fn stamp_of(dir: &Path, file: &str) -> Stamp {
    let meta = fs::symlink_metadata(dir.join(file)).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// The id a new record gets.
fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(super) fn channel_log_path(dir: &Path, channel_id: &str) -> Result<PathBuf, StoreError> {
    if !safe_chat_id(channel_id) {
        return Err(StoreError::refused(CHANNELS_FILE, "Unknown channel"));
    }
    Ok(dir
        .join(CHANNEL_LOGS_DIR)
        .join(format!("{channel_id}.jsonl")))
}

pub(super) fn sorted_visions(visions: &BTreeMap<String, Vision>) -> Vec<Vision> {
    let mut list: Vec<Vision> = visions.values().cloned().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    list
}

pub(super) fn project_channels(channels: &BTreeMap<String, Channel>, root: &Path) -> Vec<Channel> {
    let mut list: Vec<Channel> = channels
        .values()
        .filter(|channel| channel.project_root == root)
        .cloned()
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// `text` trimmed, when it has at most `max_chars` characters (and some, unless `empty_ok`).
fn bounded(text: &str, max_chars: usize, empty_ok: bool) -> Option<String> {
    let text = text.trim();
    (text.chars().count() <= max_chars && (empty_ok || !text.is_empty())).then(|| text.to_owned())
}

fn validate_custom(custom: Option<&BTreeMap<String, bool>>) -> Result<(), &'static str> {
    for (row, allowed) in custom.into_iter().flatten() {
        match Decision::from_id(row) {
            None => return Err("Unknown autonomy row"),
            Some(Decision::External | Decision::Spend) if *allowed => {
                return Err("Sending work out and spending are always the person's decisions")
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn validate_vision(spec: &mut Vision) -> Result<(), &'static str> {
    if !spec.id.is_empty() && !safe_chat_id(&spec.id) {
        return Err("Unknown Vision");
    }
    spec.name = bounded(&spec.name, MAX_NAME_CHARS, false)
        .ok_or("A Vision needs a name of at most 60 characters")?;
    spec.role = bounded(&spec.role, MAX_ROLE_CHARS, true)
        .ok_or("A Vision's role is at most 200 characters")?;
    spec.shape = bounded(&spec.shape, MAX_LOOK_CHARS, false)
        .ok_or("A Vision needs a shape of at most 40 characters")?;
    spec.color = bounded(&spec.color, MAX_LOOK_CHARS, false)
        .ok_or("A Vision needs a colour of at most 40 characters")?;
    spec.model = match spec.model.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(model) if model.chars().count() <= MAX_MODEL_CHARS => Some(model.to_owned()),
        Some(_) => return Err("A Vision's model name is at most 200 characters"),
    };
    validate_custom(spec.custom.as_ref())?;
    if spec
        .daily_budget_usd
        .is_some_and(|budget| !budget.is_finite() || budget < 0.0)
    {
        return Err("A daily budget is a positive number of dollars");
    }
    if spec.soul.len() > SOUL_MAX_BYTES {
        return Err("A Vision's soul is at most 4 KiB");
    }
    Ok(())
}

fn valid_channel_name(name: &str) -> bool {
    (1..=MAX_CHANNEL_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_channel(spec: &mut Channel) -> Result<(), &'static str> {
    if !spec.id.is_empty() && !safe_chat_id(&spec.id) {
        return Err("Unknown channel");
    }
    if !valid_channel_name(&spec.name) {
        return Err("A channel name is 1 to 40 lowercase letters, digits or dashes");
    }
    spec.topic = bounded(&spec.topic, MAX_TOPIC_CHARS, true)
        .ok_or("A channel topic is at most 500 characters")?;
    if !spec.project_root.is_absolute() {
        return Err("A channel's project folder must be an absolute path");
    }
    let mut members: Vec<String> = Vec::new();
    for member in spec.members.drain(..) {
        if !members.contains(&member) {
            members.push(member);
        }
    }
    if members.len() > MAX_MEMBERS {
        return Err("A channel has at most 32 members");
    }
    spec.members = members;
    Ok(())
}

/// Fails unless the record was opened at its current revision (None for a new record).
fn check_revision(
    file: &'static str,
    current: Option<&str>,
    expected: Option<&str>,
    stale: &str,
) -> Result<(), StoreError> {
    if current == expected {
        Ok(())
    } else {
        Err(StoreError::refused(file, stale))
    }
}

/// The records sorted by key, so a file changes only where its records do.
fn by_key<'a, T: 'a>(records: impl Iterator<Item = (&'a String, &'a T)>) -> Vec<&'a T> {
    let mut records: Vec<(&String, &T)> = records.collect();
    records.sort_by(|a, b| a.0.cmp(b.0));
    records.into_iter().map(|(_, record)| record).collect()
}

impl VisionsCache {
    /// Fails, and drops the loaded copy so the next call reads the file again, when `file`
    /// changed on disk since it was read or last written here.
    fn ensure_unchanged(&mut self, dir: &Path, file: &'static str) -> Result<(), StoreError> {
        let Some(stamp) = self.stamps.get(file) else {
            return Ok(());
        };
        if *stamp == stamp_of(dir, file) {
            return Ok(());
        }
        self.stamps.remove(file);
        match file {
            VISIONS_FILE => self.visions = None,
            CHANNELS_FILE => self.channels = None,
            _ => self.owners = None,
        }
        let message = format!(
            "visions/{file} changed on disk since it was read; it was left as it is, so reload and save again"
        );
        Err(StoreError::refused(file, &message))
    }

    fn restamp(&mut self, dir: &Path, file: &'static str) {
        self.stamps.insert(file, stamp_of(dir, file));
    }

    pub(super) fn visions(
        &mut self,
        dir: &Path,
    ) -> Result<&mut BTreeMap<String, Vision>, StoreError> {
        if self.visions.is_none() {
            let stamp = stamp_of(dir, VISIONS_FILE);
            let list: Vec<Vision> = read_store(dir, VISIONS_FILE)?;
            self.stamps.insert(VISIONS_FILE, stamp);
            self.visions = Some(list.into_iter().map(|v| (v.id.clone(), v)).collect());
        }
        Ok(self.visions.get_or_insert_with(BTreeMap::new))
    }

    pub(super) fn channels(
        &mut self,
        dir: &Path,
    ) -> Result<&mut BTreeMap<String, Channel>, StoreError> {
        if self.channels.is_none() {
            let stamp = stamp_of(dir, CHANNELS_FILE);
            let list: Vec<Channel> = read_store(dir, CHANNELS_FILE)?;
            self.stamps.insert(CHANNELS_FILE, stamp);
            self.channels = Some(list.into_iter().map(|c| (c.id.clone(), c)).collect());
        }
        Ok(self.channels.get_or_insert_with(BTreeMap::new))
    }

    pub(super) fn owners(&mut self, dir: &Path) -> Result<&mut HashMap<String, Owner>, StoreError> {
        if self.owners.is_none() {
            let stamp = stamp_of(dir, OWNERS_FILE);
            let list: Vec<Owner> = read_store(dir, OWNERS_FILE)?;
            self.stamps.insert(OWNERS_FILE, stamp);
            self.owners = Some(list.into_iter().map(|o| (o.thread_id.clone(), o)).collect());
        }
        Ok(self.owners.get_or_insert_with(HashMap::new))
    }

    pub(super) fn save_vision(
        &mut self,
        dir: &Path,
        mut spec: Vision,
        expected: Option<&str>,
    ) -> Result<Vision, StoreError> {
        validate_vision(&mut spec).map_err(|message| StoreError::refused(VISIONS_FILE, message))?;
        self.ensure_unchanged(dir, VISIONS_FILE)?;
        let visions = self.visions(dir)?;
        let current = if spec.id.is_empty() {
            spec.id = new_id();
            None
        } else {
            let previous = visions
                .get(&spec.id)
                .ok_or_else(|| StoreError::refused(VISIONS_FILE, "Unknown Vision"))?;
            Some(previous.revision.as_str())
        };
        let stale = "Vision changed since it was opened; reload before saving";
        check_revision(VISIONS_FILE, current, expected, stale)?;
        spec.revision = next_revision(current);
        let mut all = by_key(visions.iter().filter(|(id, _)| **id != spec.id));
        all.push(&spec);
        write_store(dir, VISIONS_FILE, &all)?;
        visions.insert(spec.id.clone(), spec.clone());
        self.restamp(dir, VISIONS_FILE);
        Ok(spec)
    }

    pub(super) fn save_channel(
        &mut self,
        dir: &Path,
        mut spec: Channel,
        expected: Option<&str>,
    ) -> Result<Channel, StoreError> {
        let refused = |message| StoreError::refused(CHANNELS_FILE, message);
        validate_channel(&mut spec).map_err(refused)?;
        if !spec.members.is_empty() {
            let visions = self.visions(dir)?;
            if spec
                .members
                .iter()
                .any(|member| !visions.contains_key(member))
            {
                return Err(refused("Channel members must be saved Visions"));
            }
        }
        self.ensure_unchanged(dir, CHANNELS_FILE)?;
        let channels = self.channels(dir)?;
        let current = if spec.id.is_empty() {
            spec.id = new_id();
            None
        } else {
            let previous = channels
                .get(&spec.id)
                .ok_or_else(|| refused("Unknown channel"))?;
            Some(previous.revision.as_str())
        };
        let stale = "Channel changed since it was opened; reload before saving";
        check_revision(CHANNELS_FILE, current, expected, stale)?;
        let taken = channels.values().any(|other| {
            other.id != spec.id
                && other.project_root == spec.project_root
                && other.name == spec.name
        });
        if taken {
            return Err(refused("This project already has a channel with that name"));
        }
        spec.revision = next_revision(current);
        let mut all = by_key(channels.iter().filter(|(id, _)| **id != spec.id));
        all.push(&spec);
        write_store(dir, CHANNELS_FILE, &all)?;
        channels.insert(spec.id.clone(), spec.clone());
        self.restamp(dir, CHANNELS_FILE);
        Ok(spec)
    }

    pub(super) fn set_owner(&mut self, dir: &Path, owner: Owner) -> Result<(), StoreError> {
        let ids = [&owner.thread_id, &owner.vision_id, &owner.channel_id];
        if !ids.iter().all(|id| safe_chat_id(id)) {
            return Err(StoreError::refused(
                OWNERS_FILE,
                "Unknown chat, Vision or channel",
            ));
        }
        self.ensure_unchanged(dir, OWNERS_FILE)?;
        let owners = self.owners(dir)?;
        if owners.get(&owner.thread_id) == Some(&owner) {
            return Ok(());
        }
        let mut all = by_key(owners.iter().filter(|(id, _)| **id != owner.thread_id));
        all.push(&owner);
        write_store(dir, OWNERS_FILE, &all)?;
        owners.insert(owner.thread_id.clone(), owner);
        self.restamp(dir, OWNERS_FILE);
        Ok(())
    }

    /// Removes a Vision and takes it out of every channel it was a member of.
    pub(super) fn delete_vision(&mut self, dir: &Path, vision_id: &str) -> Result<(), StoreError> {
        self.ensure_unchanged(dir, VISIONS_FILE)?;
        self.ensure_unchanged(dir, CHANNELS_FILE)?;
        if !self.visions(dir)?.contains_key(vision_id) {
            return Err(StoreError::refused(VISIONS_FILE, "Unknown Vision"));
        }
        let channels = self.channels(dir)?;
        if channels
            .values()
            .any(|channel| channel.members.iter().any(|member| member == vision_id))
        {
            let mut updated = channels.clone();
            for channel in updated.values_mut() {
                if channel.members.iter().any(|member| member == vision_id) {
                    channel.members.retain(|member| member != vision_id);
                    channel.revision = next_revision(Some(&channel.revision));
                }
            }
            write_store(dir, CHANNELS_FILE, &by_key(updated.iter()))?;
            *channels = updated;
            self.restamp(dir, CHANNELS_FILE);
        }
        let visions = self.visions(dir)?;
        let all = by_key(visions.iter().filter(|(id, _)| id.as_str() != vision_id));
        write_store(dir, VISIONS_FILE, &all)?;
        visions.remove(vision_id);
        self.restamp(dir, VISIONS_FILE);
        Ok(())
    }

    /// Removes a channel; its log stays on disk.
    pub(super) fn delete_channel(
        &mut self,
        dir: &Path,
        channel_id: &str,
    ) -> Result<(), StoreError> {
        self.ensure_unchanged(dir, CHANNELS_FILE)?;
        let channels = self.channels(dir)?;
        if !channels.contains_key(channel_id) {
            return Err(StoreError::refused(CHANNELS_FILE, "Unknown channel"));
        }
        let all = by_key(channels.iter().filter(|(id, _)| id.as_str() != channel_id));
        write_store(dir, CHANNELS_FILE, &all)?;
        channels.remove(channel_id);
        self.restamp(dir, CHANNELS_FILE);
        Ok(())
    }

    /// Sets the hold on `thread_id`'s owner, when it has one, and returns the owner as saved.
    pub(super) fn set_hold(
        &mut self,
        dir: &Path,
        thread_id: &str,
        hold: impl FnOnce(&Owner) -> super::Hold,
    ) -> Result<Option<Owner>, StoreError> {
        let Some(mut owner) = self.owners(dir)?.get(thread_id).cloned() else {
            return Ok(None);
        };
        owner.hold = hold(&owner);
        self.set_owner(dir, owner.clone())?;
        Ok(Some(owner))
    }

    pub(super) fn release_owner(&mut self, dir: &Path, thread_id: &str) -> Result<(), StoreError> {
        self.ensure_unchanged(dir, OWNERS_FILE)?;
        let owners = self.owners(dir)?;
        if !owners.contains_key(thread_id) {
            return Ok(());
        }
        let all = by_key(owners.iter().filter(|(id, _)| id.as_str() != thread_id));
        write_store(dir, OWNERS_FILE, &all)?;
        owners.remove(thread_id);
        self.restamp(dir, OWNERS_FILE);
        Ok(())
    }
}
