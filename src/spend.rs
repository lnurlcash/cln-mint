//! LUD-25 spends: which note a `k1` names, and whether it opens it here.
//!
//! Decoding, the key path, the bearer hashlock, leaf rules and time claims
//! are `lnurlcash-core`'s. Any other tapscript leaf is handed to Bitcoin
//! Core's own interpreter when `cln-mint-bitcoinkernel` loads it (see
//! `kernel.rs`); without it such a spend is refused rather than guessed at.

use lnurlcash_core::spend::{Spend, SpendVerdict, check_spend, check_time_claim, decode_spend};

pub use lnurlcash_core::spend::decode_note;

use crate::kernel::Kernel;

/// What a key-path failure, an unknown note and a spent note all look like
/// from outside: explaining a failed signature only helps someone guess.
pub const INVALID_K1: &str = crate::db::INVALID_K1;

/// A decoded `k1`: the note it names.
#[derive(Debug, Clone)]
pub struct ParsedK1 {
    pub note_id: String,
    k1: String,
    spend: Spend,
}

/// hex(Q) of whatever goes where a `cp1` goes: a `cp1` or a bearer `h`.
pub fn note_id_of_ref(value: &str) -> Option<String> {
    decode_note(value).map(hex::encode)
}

/// The note `k1` claims to spend, or `None` if it is no current-format spend.
/// A 65-byte legacy `ck1` is refused: it binds to no domain.
pub fn parse(k1: &str) -> Option<ParsedK1> {
    let spend = decode_spend(k1)?;
    if matches!(spend, Spend::LegacyKeyPath { .. }) {
        return None;
    }
    Some(ParsedK1 {
        note_id: hex::encode(spend.output_key()),
        k1: k1.trim().to_string(),
        spend,
    })
}

/// `None` if `parsed` opens its note at any of `domains` at time `now`, else
/// why not. `locked_at` is when the mint credited the note.
pub fn verify(
    parsed: &ParsedK1,
    locked_at: u64,
    domains: &[String],
    now: u64,
    kernel: Option<&Kernel>,
) -> Option<String> {
    let mut reason = None;
    for domain in domains {
        let verdict = match check_spend(&parsed.k1, domain) {
            Some(check) => check.verdict,
            None => return Some(INVALID_K1.into()),
        };
        let verdict = match (verdict, &parsed.spend) {
            (SpendVerdict::Unevaluated, Spend::ScriptPath { output_key, cw1 }) => match kernel {
                Some(kernel) => match kernel.verify_script_path(output_key, domain, cw1) {
                    Ok(true) => SpendVerdict::Opens,
                    Ok(false) => {
                        SpendVerdict::Fails("the witness does not satisfy the leaf".into())
                    }
                    Err(e) => {
                        log::warn!("libbitcoinkernel: {e:#}");
                        SpendVerdict::Unevaluated
                    }
                },
                None => SpendVerdict::Unevaluated,
            },
            (verdict, _) => verdict,
        };
        match verdict {
            SpendVerdict::Opens => {
                reason = None;
                break;
            }
            // only a legacy ck1 opens as legacy, and parse refused those
            SpendVerdict::OpensLegacy => reason = Some(INVALID_K1.into()),
            SpendVerdict::Fails(why) => reason = Some(why),
            SpendVerdict::Unevaluated => {
                reason = Some("this mint cannot evaluate this script".into())
            }
        }
    }
    if domains.is_empty() {
        return Some(INVALID_K1.into());
    }
    if let Some(reason) = reason {
        // a key path says nothing more than "invalid"; a cw1 discloses its
        // whole secret already, so its reason can't help anyone guess
        return Some(match parsed.spend {
            Spend::KeyPath { .. } => INVALID_K1.into(),
            _ => reason,
        });
    }
    if let Spend::ScriptPath { cw1, .. } = &parsed.spend {
        return check_time_claim(cw1.locktime, cw1.sequence, now, locked_at);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lnurlcash_core::recoverable::{
        derive_note_pubkey, derive_note_secret_key, encode_ck1, encode_cp1, encode_cw1,
        sign_note_ownership,
    };
    use lnurlcash_core::spend::bearer_cw1;
    use sha2::{Digest, Sha256};

    fn domains() -> Vec<String> {
        vec!["mint.example".into(), "abc.onion".into()]
    }

    #[test]
    fn bearer_short_forms_agree() {
        let preimage = [7u8; 32];
        let h = Sha256::digest(preimage);
        let parsed = parse(&hex::encode(preimage)).unwrap();
        assert_eq!(
            Some(parsed.note_id.clone()),
            note_id_of_ref(&hex::encode(h))
        );
        assert_eq!(verify(&parsed, 0, &domains(), 1, None), None);
        // the full cw1 names the same note and opens it too
        let cw1 = encode_cw1(&bearer_cw1(&preimage).unwrap()).unwrap();
        let full = parse(&cw1).unwrap();
        assert_eq!(full.note_id, parsed.note_id);
        assert_eq!(verify(&full, 0, &domains(), 1, None), None);
        let q: [u8; 32] = hex::decode(&parsed.note_id).unwrap().try_into().unwrap();
        assert_eq!(note_id_of_ref(&encode_cp1(&q)), Some(parsed.note_id));
    }

    #[test]
    fn key_path_binds_to_our_domains_only() {
        let secret = [3u8; 32];
        let ck1_here = encode_ck1(&sign_note_ownership(&secret, "abc.onion").unwrap());
        let ck1_elsewhere = encode_ck1(&sign_note_ownership(&secret, "other.example").unwrap());
        let here = parse(&ck1_here).unwrap();
        let elsewhere = parse(&ck1_elsewhere).unwrap();
        assert_eq!(here.note_id, elsewhere.note_id);
        assert_eq!(verify(&here, 0, &domains(), 1, None), None);
        assert_eq!(
            verify(&elsewhere, 0, &domains(), 1, None).as_deref(),
            Some(INVALID_K1)
        );
    }

    #[test]
    fn derived_keys_match_cx1_derivation() {
        let branch_sk = [9u8; 32];
        let chain = [1u8; 32];
        let secp = secp256k1::Secp256k1::new();
        let kp = secp256k1::Keypair::from_seckey_slice(&secp, &branch_sk).unwrap();
        let p = kp.x_only_public_key().0.serialize();
        let sk = derive_note_secret_key(&branch_sk, &chain, 2, 5).unwrap();
        let q = derive_note_pubkey(&p, &chain, 2, 5).unwrap();
        let ck1 = encode_ck1(&sign_note_ownership(&sk, "mint.example").unwrap());
        assert_eq!(parse(&ck1).unwrap().note_id, hex::encode(q));
    }

    #[test]
    fn timelocks_use_the_mints_clock() {
        let preimage = [5u8; 32];
        let mut cw1 = bearer_cw1(&preimage).unwrap();
        cw1.locktime = 1_700_000_000;
        let parsed = parse(&encode_cw1(&cw1).unwrap()).unwrap();
        assert!(verify(&parsed, 0, &domains(), 1_600_000_000, None).is_some());
        assert_eq!(verify(&parsed, 0, &domains(), 1_800_000_000, None), None);
    }

    #[test]
    fn garbage_is_no_spend() {
        assert!(parse("hello").is_none());
        assert!(parse("").is_none());
    }
}
