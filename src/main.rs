fn main() {

    cmd_lib::run_cmd! (
       cat "file/*" > "2.txt";
    );
    println!("Hello, world!");
}
