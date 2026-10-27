//! The Rust VOC Index against Sensirion's reference C, sample for sample.
//! `tests/gas_index_ref/gen.sh` rebuilds expected.csv from the C sources.

use env_hat::gas_index::VocIndex;

/// Feeds each `sraw,index` line to `voc` and asserts it gives `index`, for
/// exactly `samples` lines; `t0` is the time of the first, for messages.
fn assert_matches_c<'a>(voc: &mut VocIndex, lines: impl Iterator<Item = &'a str>, t0: usize, samples: usize) {
    let mut seen = 0;
    let mut mismatches = Vec::new();
    for (t, line) in lines.enumerate() {
        let (sraw, want) = line.split_once(',').unwrap();
        let got = voc.process(sraw.parse().unwrap());
        if got != want.parse::<i32>().unwrap() {
            mismatches.push(format!("t={} sraw={sraw}: C {want}, Rust {got}", t0 + t + 1));
        }
        seen += 1;
    }
    assert_eq!(seen, samples);
    assert!(mismatches.is_empty(), "{} of {seen} differ:\n{}", mismatches.len(), mismatches.join("\n"));
}

#[test]
fn matches_sensirion_reference_c() {
    let csv = include_str!("gas_index_ref/expected.csv");
    assert_matches_c(&mut VocIndex::default(), csv.lines().skip(1), 0, 14 * 3600);
}

#[test]
fn restored_state_matches_sensirion_reference_c() {
    let mut csv = include_str!("gas_index_ref/expected_states.csv").lines();
    let state: Vec<&str> = csv.next().unwrap().split(',').collect();
    let (mean, std): (f32, f32) = (state[1].parse().unwrap(), state[2].parse().unwrap());

    let mut before = VocIndex::default();
    let full = include_str!("gas_index_ref/expected.csv");
    for line in full.lines().skip(1).take(4 * 3600) {
        before.process(line.split_once(',').unwrap().0.parse().unwrap());
    }
    assert_eq!(before.state(), (mean, std), "get_states after 4 h");

    let mut after = VocIndex::default();
    after.set_state(mean, std);
    assert_matches_c(&mut after, csv.skip(1), 4 * 3600, 2 * 3600);
}
