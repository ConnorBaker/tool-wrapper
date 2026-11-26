use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::ExitCode;
use tool_wrapper::{artifact::normalize_archive_metadata, wrap_tool};

fn main() -> ExitCode {
    wrap_tool!(Some("ar"), |tool| {
        // Demonstration policy for one command form. GNU ar also supports deterministic mode.
        if let [operation, output, _, ..] = tool.args.as_slice()
            && operation == b"crU"
        {
            let output = PathBuf::from(OsStr::from_bytes(output));
            tool.edit_output(tool.command(&tool.args), output, normalize_archive_metadata)
        } else {
            Ok(tool.forward())
        }
    })
}
