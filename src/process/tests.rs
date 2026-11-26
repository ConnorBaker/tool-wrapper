use super::*;
use std::{env, fs, os::unix::ffi::OsStringExt};

const PROBE: [&str; 2] = ["--exact", "process::tests::probe_staged_input"];
const STAGED_PATH: &str = "TOOL_WRAPPER_TEST_STAGED_PATH";
const EXPECT_STAGED: &str = "TOOL_WRAPPER_TEST_EXPECT_STAGED";

#[test]
fn probe_staged_input() -> io::Result<()> {
    let Some(path) = env::var_os(STAGED_PATH) else {
        return Ok(());
    };
    let present = fs::read_link(&path)
        .is_ok_and(|link| link.as_os_str().as_bytes() == b"/memfd:descriptor-isolation (deleted)");
    assert_eq!(present, env::var_os(EXPECT_STAGED).is_some());
    if present {
        assert_eq!(fs::read(path)?, b"staged input");
    }
    Ok(())
}

#[test]
fn only_prepared_commands_inherit_staged_input() -> io::Result<()> {
    let mut inputs = InputFiles::default();
    let path = OsString::from_vec(inputs.stage(c"descriptor-isolation", b"staged input")?);
    let executable = env::current_exe()?;
    let tool = Tool {
        name: Cow::Borrowed(OsStr::new("probe")),
        args: PROBE.map(|arg| arg.as_bytes().to_vec()).into(),
        launcher_options: Vec::new(),
        argv0: executable.clone().into_os_string(),
        vendor: executable.clone(),
        inputs,
    };
    let mut prepared = tool.command(&tool.args);
    // The command owns the staged input even after the Tool is dropped.
    drop(tool);
    let unrelated = Command::new(executable)
        .args(PROBE)
        .env(STAGED_PATH, &path)
        .env_remove(EXPECT_STAGED)
        .output()?;
    assert!(unrelated.status.success(), "{unrelated:?}");
    let prepared = prepared
        .env(STAGED_PATH, path)
        .env(EXPECT_STAGED, "")
        .output()?;
    assert!(prepared.status.success(), "{prepared:?}");
    Ok(())
}
