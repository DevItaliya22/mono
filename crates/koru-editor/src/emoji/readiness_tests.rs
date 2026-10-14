// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn empty_query_offers_the_catalog_before_the_worker_publishes_a_snapshot() {
    // A newly started worker has no published candidates. Model that state
    // directly so this proof does not depend on winning a scheduler race.
    let mut search = EmojiSearch {
        worker: Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1),
        query: String::new(),
    };
    assert_eq!(search.worker.snapshot().matched_item_count(), 0);
    let mut hits = search.results(catalog().len());
    hits.sort_unstable();
    assert_eq!(hits, (0..catalog().len()).collect::<Vec<_>>());
}
