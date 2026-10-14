// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn an_empty_table_converts_nothing_and_says_so() {
    let table = T2sTable::empty();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert_eq!(table.convert("開啟文檔"), "開啟文檔");
}

#[test]
fn a_character_table_converts_character_by_character() {
    let table = T2sTable::parse("開\t开\n啟\t启\n檔\t档\n").expect("parses");
    assert_eq!(table.len(), 3);
    assert_eq!(table.phrases(), 0);
    assert_eq!(table.convert("開啟文檔"), "开启文档");
}

#[test]
fn a_longer_key_wins_over_its_characters() {
    // How a phrase table is expressed in this format. OpenCC's `t2s` is
    // `TSCharacters` plus `TSPhrases`, and the phrase table is what handles the
    // characters whose Simplified form depends on context.
    let table = T2sTable::parse("乾\t干\n乾坤\t乾坤\n").expect("parses");
    assert_eq!(table.phrases(), 1);
    assert_eq!(table.convert("乾"), "干");
    assert_eq!(table.convert("乾坤"), "乾坤");
    assert_eq!(table.convert("乾坤乾"), "乾坤干");
}

#[test]
fn comments_and_blank_lines_are_skipped() {
    let table =
        T2sTable::parse("# opencc t2s, dumped by the corpus step\n\n開\t开\n").expect("parses");
    assert_eq!(table.len(), 1);
}

#[test]
fn a_line_with_no_tab_is_an_error_rather_than_a_skip() {
    // A normalisation table that silently ignored half of itself would produce
    // a number nobody could reproduce, which is the failure the whole pinning
    // section of the protocol exists to prevent.
    let error = T2sTable::parse("開 开\n").expect_err("refused");
    assert!(error.contains("line 1"));
}

#[test]
fn a_duplicate_key_is_an_error() {
    let error = T2sTable::parse("開\t开\n開\t啓\n").expect_err("refused");
    assert!(error.contains("line 2"));
    assert!(error.contains("開"));
}

#[test]
fn an_empty_key_is_an_error() {
    assert!(T2sTable::parse("\t开\n").is_err());
}

#[test]
fn a_simplified_string_passes_through_unchanged() {
    let table = T2sTable::parse("開\t开\n").expect("parses");
    assert_eq!(table.convert("打开文档"), "打开文档");
}

#[test]
fn the_file_name_is_the_one_the_corpus_step_writes() {
    // Two places have to agree about this string: the script that dumps the
    // table and the harness that reads it.
    assert_eq!(T2S_FILE, "opencc-t2s.tsv");
    assert!(T2S_PAIRS.contains("traditional"));
}
