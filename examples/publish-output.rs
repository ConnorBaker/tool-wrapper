use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use tool_wrapper::{artifact::normalize_archive_metadata, wrap_tool};

fn main() -> ExitCode {
    wrap_tool!(Some("cp"), |tool| {
        if let [input, output] = tool.args.as_slice()
            && !input.starts_with(b"-")
            && !output.starts_with(b"-")
        {
            let output = Path::new(OsStr::from_bytes(output));
            tool.publish_output(
                output,
                |staged| Ok(tool.command_from([input.as_slice(), staged.as_os_str().as_bytes()])),
                normalize_archive_metadata,
            )
        } else {
            Ok(tool.forward())
        }
    })
}
