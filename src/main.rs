use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let bin_name = args.first().cloned().unwrap_or_default();

    if bin_name.ends_with("rsupervisorctl") || bin_name.ends_with("rsupervisorctl.exe") {
        return rsupervisord::cli::run().await;
    }

    if args.len() > 1 && args[1] == "ctl" {
        // Dispatch "rsupervisord ctl ..." to CLI
        args.remove(1);
        let parsed = rsupervisord::cli::CliArgs::parse_from(args);
        return rsupervisord::cli::run_with_args(parsed).await;
    }

    println!("rsupervisord daemon starting...");
    Ok(())
}
