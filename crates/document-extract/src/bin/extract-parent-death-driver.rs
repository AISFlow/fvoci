//! Feature-gated parent-death fixture. Production builds omit this binary
//! (`required-features = ["test-hang"]`). It is not a daemon.

fn main() {
    let mut args = std::env::args().skip(1);
    let extractor = args.next().expect("extractor bin path");
    let pid_file = args.next().expect("helper pid file path");
    document_extract::process::run_parent_death_driver(
        std::path::PathBuf::from(extractor),
        std::path::PathBuf::from(pid_file),
    );
}
