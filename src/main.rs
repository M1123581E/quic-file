fn main() {

    let cmd = format!("cat {} > 2.txt", "file/*");
    cmd_lib::run_cmd! (
       sudo bash -c ${cmd};
    );
    println!("Hello, world!");
}
