use std::mem;
use std::process::ExitCode;
use tool_wrapper::{
    response::{Gnu, Limits, rewrite},
    wrap_tool,
};

fn main() -> ExitCode {
    wrap_tool!(Some("ar"), |tool| {
        let mut operation = true;
        let rewritten = rewrite(&tool.args, Gnu, Limits::default(), |word, _| {
            // Select deterministic mode for the demonstration's operation.
            Ok((mem::take(&mut operation) && word == b"crU").then(|| b"crD".to_vec()))
        })?;
        Ok(tool.with_prepared_args(rewritten).forward())
    })
}
