use std::process::ExitCode;
use tool_wrapper::wrap_tool;

fn main() -> ExitCode {
    wrap_tool!(None, |tool| Ok(tool.forward()))
}
