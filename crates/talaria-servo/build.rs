use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};

const FILTER_LISTS: [&str; 6] = [
    "resources/adblock/ublock-filters.txt",
    "resources/adblock/easylist.txt",
    "resources/adblock/easyprivacy.txt",
    "resources/adblock/brave-firstparty.txt",
    "resources/adblock/brave-specific.txt",
    "resources/adblock/brave-unbreak.txt",
];

fn main() {
    let mut filter_set = FilterSet::new(false);
    let parse_options = ParseOptions {
        rule_types: RuleTypes::NetworkOnly,
        ..ParseOptions::default()
    };

    for path in FILTER_LISTS {
        println!("cargo:rerun-if-changed={path}");
        let rules = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("failed to read adblock list {path}: {error}"));
        filter_set.add_filter_list(rules, parse_options);
    }

    let engine = Engine::new_with_filter_set(filter_set);
    let output_path = output_directory().join("adblock-engine.dat");
    fs::write(&output_path, engine.serialize()).unwrap_or_else(|error| {
        panic!(
            "failed to write compiled adblock engine {}: {error}",
            output_path.display()
        )
    });
}

fn output_directory() -> PathBuf {
    Path::new(&env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR")).to_path_buf()
}
