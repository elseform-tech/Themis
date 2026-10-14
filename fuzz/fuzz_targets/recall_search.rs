//! Saved snapshots and attachments are untrusted: any bytes as a JSONL file and
//! any query must keep `recall` panic-free, bounded, and its ranges in bounds.
#![no_main]
use libfuzzer_sys::{arbitrary, fuzz_target};
use themis_core::tools::recall::{
    read_line_capped, record_fields, recall, Limits, Line, ReadLimit, RecallArgs, RecallScope, EXCERPT_CHARS,
    MAX_LIMIT,
};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input<'a> {
    snapshot: &'a [u8],
    query: &'a str,
    source: &'a str,
    around: &'a str,
    budget: u16,
}

fuzz_target!(|input: Input| {
    let mut cursor = std::io::Cursor::new(input.snapshot);
    let limit = ReadLimit { max_bytes: u64::from(input.budget), deadline: None };
    let (mut line, mut bytes) = (Vec::new(), 0u64);
    while let Ok(Line::Complete | Line::Oversize) = read_line_capped(&mut cursor, &mut line, 4096, &mut bytes, &limit) {
        assert!(line.len() <= 4096);
        assert!(bytes <= u64::from(input.budget));
        for field in record_fields(&line) {
            assert!(field.pointer.starts_with('/'));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let evidence = dir.path().join("e");
    std::fs::create_dir(&evidence).unwrap();
    std::fs::write(evidence.join("snapshot-1.jsonl"), input.snapshot).unwrap();
    let scope = RecallScope {
        evidence_dir: evidence,
        attachments: Vec::new(),
        attachment_root: None,
        project_root: Some(dir.path().to_path_buf()),
    };
    let args = RecallArgs {
        query: input.query.into(),
        source: input.source.into(),
        around: input.around.into(),
        ..RecallArgs::default()
    };
    if let Ok(result) = recall(&scope, &args, &Limits::default()) {
        if let Some(hits) = result["hits"].as_array() {
            assert!(hits.len() <= MAX_LIMIT);
            for hit in hits {
                assert!(hit["excerpt"].as_str().unwrap().chars().count() <= EXCERPT_CHARS);
            }
        }
    }
});
