#[path = "../../../bench/workloads/cssom_flat.rs"]
mod workload;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    workload::run()
}
