# Additive correction to the initial expectations

The signed final explicitly requires region_unavailable for an unsupported
account home region. Initial cases.json incorrectly classified the EU trusted
bundle and the EU authenticated response as unconfigured/mismatch. Its bytes
remain frozen. erratum-1.json explicitly overrides only region_eu and
hostile_region, and adds unknown-region, duplicate-region and wrong-account
unsupported-region cases. A structured error must be produced after strict
decoding, never by parsing serde error prose or first decoding through Value.

An unrelated review found that percent-encoding a Unicode relay URL can expand
an accepted raw string beyond the same wire limit on reparse. The additional
relay_expansion input must refuse; both raw and canonical lengths are bounded.
The generated route schema must require relay_url while allowing string/null,
including the nested seed route. These are corrections against the signed
contract, not a new wire version or an amendment of its authority.
