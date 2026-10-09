#[path = "../../../bench/workloads/qjs_domstring_lossy.rs"]
mod workload;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    workload::run()
}
