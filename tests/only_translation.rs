//! `-S` stops after translation: the same labels and dump as a full run, no simulation.

use wrench::config::Config;
use wrench::isa::risc_iv::RiscIv;
use wrench::wrench::{wrench, Options};

#[test]
fn only_translation_skips_simulation() {
    let path = "example/risc-iv-32/factorial.s";
    let src = std::fs::read_to_string(path).unwrap();
    let conf = Config::read("example/risc-iv-32/factorial-5.yaml").unwrap();
    let full_opts = Options {
        input: path.to_string(),
        ..Options::default()
    };
    let only_opts = Options {
        only_translation: true,
        ..full_opts.clone()
    };

    let full = wrench::<RiscIv>(&full_opts, &conf, &src).unwrap();
    let only = wrench::<RiscIv>(&only_opts, &conf, &src).unwrap();

    assert!(!full.trace.is_empty());
    assert!(only.trace.is_empty());
    assert!(only.success);
    assert_eq!(only.labels, full.labels);
    let addrs = |r: &wrench::wrench::WrenchResult<_>| r.dump.iter().map(|(a, _)| *a).collect::<Vec<_>>();
    assert_eq!(addrs(&only), addrs(&full));
}
