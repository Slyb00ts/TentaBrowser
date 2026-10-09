#[path = "../../../bench/workloads/shared_parse.rs"]
mod shared_parse;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    shared_parse::run()
}
