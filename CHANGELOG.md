# Changelog

## [0.1.0] - unreleased

### Added

- LUD-25 mint: minting from a payRequest (`cp1` or bearer `h` as the LUD-12 comment), the informational withdrawRequest (`?k1=` and `?p=`), melt, rotate, split and merge, retried mutations answered with their original result, `cs1` certificates signed with the node's `signmessage`
- LUD-26: Lightning Address registration against a `cx1` branch with BIP-340 ownership proofs, auto-mint onto purpose 2, `text/cpub` for internal transfers
- LUD-21 `verify` for mint invoices and melts, NIP-05 for registered usernames
- every spend verified by Bitcoin Core's own interpreter, compiled in through `lnurlcash-kernel`
- database compatible with lnurl-mint's
