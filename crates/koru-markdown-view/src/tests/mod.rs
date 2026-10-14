// SPDX-License-Identifier: MPL-2.0

//! Pure, every one of them: this surface touches no disk, no store and no
//! network, so its suite needs none either.

mod dependency;
mod round_trip;
mod states;

/// The canonical Markdown — every shape Koru has a node for, written the way
/// the exporter writes it. `round_trip` asserts this exact text survives the
/// document unchanged, which is what "Koru supports it" means.
pub(crate) const CANONICAL: &str = "\
# The canonical document

A paragraph with **bold**, *italic*, ~~struck~~ and `code` in it.

## A second heading

- a bullet
- another bullet

1. first
2. second

- [ ] a box to tick
- [x] a box already ticked

> a quotation

```rust
fn main() {}
```

| A | B |
| --- | --- |
| 1 | 2 |

---

A closing paragraph.
";
