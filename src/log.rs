pub fn log(msg: &str) {
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    eprintln!("[{}] {}", ts, msg);
}
