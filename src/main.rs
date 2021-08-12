fn main() {

    cmd_lib::run_cmd! (
       cat "file/*.txt" > "2.txt";
    );
    println!("Hello, world!");
}
