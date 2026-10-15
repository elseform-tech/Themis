//! macOS Seatbelt profile for `/usr/bin/sandbox-exec`.
//!
//! Paths never appear in the profile text. Each one is passed as a separate
//! `-D NAME=VALUE` argument and referenced as `(param "NAME")`, so quotes,
//! parentheses, newlines or `#"` in a path cannot change what the profile
//! means. Seatbelt matches resolved paths, so the plan's paths are canonical
//! (`/tmp` is `/private/tmp`). Later rules win over earlier ones.

use super::{NetworkPolicy, SandboxPlan};
use std::ffi::OsString;
use std::path::Path;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Device files every program may write (terminal, null sink, descriptors).
const DEVICE_LITERALS: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/tty",
    "/dev/dtracehelper",
    "/dev/random",
    "/dev/urandom",
];

/// The profile text and its `(name, path)` parameters.
pub fn profile(plan: &SandboxPlan) -> (String, Vec<(String, OsString)>) {
    build(
        plan.writable(),
        plan.protected(),
        plan.secrets(),
        plan.network() == NetworkPolicy::Block,
    )
}

/// Profile for explicit path lists (see [`profile`]).
pub fn build<P: AsRef<Path>>(
    writable: &[P],
    protected: &[P],
    secrets: &[P],
    block_network: bool,
) -> (String, Vec<(String, OsString)>) {
    let mut params = Vec::new();
    let mut text =
        String::from("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write*\n");
    for device in DEVICE_LITERALS {
        text.push_str(&format!("  (literal \"{device}\")\n"));
    }
    text.push_str(
        "  (regex #\"^/dev/fd/\")\n  (regex #\"^/dev/ttys\")\n  (regex #\"^/dev/pty\")\n",
    );
    let mut add = |text: &mut String, prefix: &str, paths: &[P]| {
        for (index, path) in paths.iter().enumerate() {
            let name = format!("{prefix}_{index}");
            text.push_str(&format!("  (subpath (param \"{name}\"))\n"));
            params.push((name, path.as_ref().as_os_str().to_owned()));
        }
    };
    add(&mut text, "WRITABLE", writable);
    text.push_str(")\n");
    if !protected.is_empty() {
        text.push_str("(deny file-write*\n");
        add(&mut text, "PROTECTED", protected);
        text.push_str(")\n");
    }
    if !secrets.is_empty() {
        text.push_str("(deny file-read* file-write*\n");
        add(&mut text, "SECRET", secrets);
        text.push_str(")\n");
    }
    if block_network {
        text.push_str(
            "(deny network-outbound (remote ip))\n(deny network-inbound (local ip))\n(deny network-bind (local ip))\n",
        );
    }
    (text, params)
}

/// `sandbox-exec` arguments that run `program args` under `profile`.
pub fn arguments(
    profile: &str,
    params: &[(String, OsString)],
    program: &Path,
    args: &[String],
) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec!["-p".into(), profile.into()];
    for (name, value) in params {
        let mut definition = OsString::from(format!("{name}="));
        definition.push(value);
        out.push("-D".into());
        out.push(definition);
    }
    out.push("--".into());
    out.push(program.as_os_str().to_owned());
    out.extend(args.iter().map(OsString::from));
    out
}
