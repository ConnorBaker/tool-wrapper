use std::process::ExitCode;
use tool_wrapper::{files::InputFiles, process::PreparedArgs, wrap_tool};

fn main() -> ExitCode {
    wrap_tool!(Some("cat"), |tool| {
        let mut inputs = InputFiles::default();
        let path = inputs.stage(c"example-input", b"staged contents\n")?;
        let mut args = tool.args.clone();
        args.push(path);
        tool.with_prepared_args(Some(PreparedArgs::new(args, inputs)))
            .supervise()
    })
}
