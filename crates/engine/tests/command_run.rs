use mediaa_core::agent::{CommandSpec, OutputFormat};
use mediaa_engine::command::{self, prepare, run};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn spec(args: Vec<String>, output: OutputFormat) -> CommandSpec {
    CommandSpec {
        program: PathBuf::from(env!("CARGO_BIN_EXE_command_stub")),
        args,
        output,
        whole_file: false,
    }
}

#[test]
fn a_failing_command_returns_stderr_and_cancel_kills_it() {
    let dir = tempfile::tempdir().unwrap();
    let err = dir.path().join("err.txt");
    std::fs::write(&err, "broken model").unwrap();
    let failing = spec(
        vec![
            "--code".into(),
            "2".into(),
            "--stderr-file".into(),
            err.display().to_string(),
        ],
        OutputFormat::WhisperJson,
    );
    let input = dir.path().join("in.wav");
    std::fs::write(&input, b"x").unwrap();
    let prepared = prepare(&failing, &input, "", "", 0.0, 1.0, &[], dir.path());
    let failed = run(&failing, &prepared, &AtomicBool::new(false)).unwrap_err();
    assert!(failed.to_string().contains("broken model"), "{failed}");
    let hanging = spec(vec!["--hang".into()], OutputFormat::WhisperJson);
    let prepared = prepare(&hanging, &input, "", "", 0.0, 1.0, &[], dir.path());
    let error = run(&hanging, &prepared, &AtomicBool::new(true)).unwrap_err();
    assert_eq!(error.kind, "cancelled");
    let _ = command::expand_home("~");
}
