use std::future::Future;
use std::pin::Pin;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};

use super::CommandResult;
use crate::err;
use crate::eval::eval;
use crate::eval::EvalContext;
use crate::eval::EvalPlan;
use crate::eval::ExecResponse;
use crate::stdio::*;

pub(super) fn wax(
    args: &[String],
    ctx: &mut EvalContext,
    _stdio: Stdio,
) -> Pin<Box<dyn Future<Output = CommandResult>>> {
    if args.len() < 2 {
        return Box::pin(async move { ExecResponse::Immediate(err::ERR_EINVAL).into() });
    }

    // Strip out the first argument
    let cmd = args[1].clone();
    let args = args.iter().skip(2).map(|a| a.clone()).collect::<Vec<_>>();
    let args = &args[..];

    // Build the script that we will execute
    let mut script = format!("wapm install {} && wapm run {}", cmd, cmd);
    for arg in args {
        script.push_str(format!(" {}", arg).as_str());
    }

    // Read the script
    let mut ctx = ctx.clone();
    return Box::pin(async move {
        ctx.input = script;

        let mut stdout = ctx.stdio.stdout.clone();
        let mut stderr = ctx.stdio.stderr.clone();

        let mut process = eval(ctx);
        let result = process.recv().await;
        drop(process);

        match result {
            Some(EvalPlan::Executed {
                code,
                ctx,
                show_result,
            }) => {
                debug!("wax executed (code={})", code);
                if code != 0 && show_result {
                    let mut chars = String::new();
                    chars += err::exit_code_to_message(code);
                    chars += "\r\n";
                    let _ = stdout.write(chars.as_bytes()).await;
                }
                let mut ret: CommandResult = ExecResponse::Immediate(code).into();
                ret.ctx = Some(ctx);
                ret
            }
            Some(EvalPlan::InternalError) => {
                debug!("wax internal error");
                let _ = stderr.write("wax: internal error\r\n".as_bytes()).await;
                ExecResponse::Immediate(err::ERR_EINTR).into()
            }
            Some(EvalPlan::MoreInput) => {
                debug!("wax more input");
                let _ = stderr
                    .write("wax: incomplete command\r\n".as_bytes())
                    .await;
                ExecResponse::Immediate(err::ERR_EINVAL).into()
            }
            Some(EvalPlan::Invalid) => {
                debug!("wax invalid");
                let _ = stderr.write("wax: invalid command\r\n".as_bytes()).await;
                ExecResponse::Immediate(err::ERR_EINVAL).into()
            }
            None => {
                debug!("wax recv error");
                let _ = stderr
                    .write(format!("wax: command failed\r\n").as_bytes())
                    .await;
                ExecResponse::Immediate(err::ERR_EINTR).into()
            }
        }
    });
}