#[path = "../../../bench/workloads/cssom_detached.rs"]
mod workload;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    workload::run()
}
