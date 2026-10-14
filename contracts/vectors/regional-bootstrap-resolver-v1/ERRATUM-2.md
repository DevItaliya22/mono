# Preserve the explicit isolated-root refusal contract

Apply after erratum-1.json. Its region_eu override incorrectly extended the
production authoritative-account distinction to isolated trusted configuration.
Restore that case's original regional_bootstrap_unconfigured result. A complete
root bundle with an unsupported response region returns
regional_bootstrap_mismatch; two added cases pin EU and unknown region. Only
unsupported home region from the current production account is the structured
region_unavailable result. No historical fixture is rewritten.
