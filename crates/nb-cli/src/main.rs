use nb_cli::{build, envelope, mcp, prescan};

#[tokio::main]
async fn main() {
    let (gate, argv) = match prescan(std::env::args().skip(1).collect()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {}", e.message);
            std::process::exit(2);
        }
    };
    let cli = build(&gate);
    let res = match gate.http {
        Some(addr) => mcp::serve_http(&cli, addr).await.map(|()| 0),
        None => envelope::run(&cli, argv).await,
    };
    match res {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
