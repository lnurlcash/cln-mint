//! Bitcoin Core's script interpreter, via `libbitcoinkernel`, for tapscript
//! leaves `lnurlcash-core` does not evaluate itself.
//!
//! Loaded at runtime from the path given by `cln-mint-bitcoinkernel`, the
//! same C API lnurl-mint reaches through `lnurlcash-kernel`. A spend is
//! checked as input 0 of LUD-25's canonical spend transaction with every
//! consensus flag LUD-25 names.

use std::ffi::c_void;

use anyhow::{Context, Result, bail};
use libloading::Library;
use lnurlcash_core::{recoverable::Cw1, spend::spend_prevout};

const FLAG_P2SH: u32 = 1 << 0;
const FLAG_DERSIG: u32 = 1 << 2;
const FLAG_NULLDUMMY: u32 = 1 << 4;
const FLAG_CHECKLOCKTIMEVERIFY: u32 = 1 << 9;
const FLAG_CHECKSEQUENCEVERIFY: u32 = 1 << 10;
const FLAG_WITNESS: u32 = 1 << 11;
const FLAG_TAPROOT: u32 = 1 << 17;
const FLAGS_ALL: u32 = FLAG_P2SH
    | FLAG_DERSIG
    | FLAG_NULLDUMMY
    | FLAG_CHECKLOCKTIMEVERIFY
    | FLAG_CHECKSEQUENCEVERIFY
    | FLAG_WITNESS
    | FLAG_TAPROOT;

type Create = unsafe extern "C" fn(*const u8, usize) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type OutputCreate = unsafe extern "C" fn(*const c_void, i64) -> *mut c_void;
type TxDataCreate = unsafe extern "C" fn(*const c_void, *const *const c_void, usize) -> *mut c_void;
type Verify = unsafe extern "C" fn(
    *const c_void,
    i64,
    *const c_void,
    *const c_void,
    u32,
    u32,
    *mut u8,
) -> i32;

#[derive(Debug)]
pub struct Kernel {
    lib: Library,
}

/// A native handle, freed exactly once.
struct Owned<'a> {
    ptr: *mut c_void,
    destroy: libloading::Symbol<'a, Destroy>,
}

impl Drop for Owned<'_> {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from the matching create call and is freed once
        unsafe { (self.destroy)(self.ptr) }
    }
}

fn compact_size(n: usize, out: &mut Vec<u8>) {
    match n {
        0..=0xfc => out.push(n as u8),
        0xfd..=0xffff => {
            out.push(0xfd);
            out.extend_from_slice(&(n as u16).to_le_bytes());
        }
        _ => {
            out.push(0xfe);
            out.extend_from_slice(&(n as u32).to_le_bytes());
        }
    }
}

/// The canonical spend transaction for `domain`, with `stack` as input 0's witness.
fn spend_tx(domain: &str, stack: &[&[u8]], locktime: u32, sequence: u32) -> Vec<u8> {
    let mut tx = Vec::new();
    tx.extend_from_slice(&2i32.to_le_bytes());
    tx.extend_from_slice(&[0x00, 0x01]); // segwit marker and flag
    tx.push(1);
    tx.extend_from_slice(&spend_prevout(domain));
    tx.extend_from_slice(&0u32.to_le_bytes());
    tx.push(0); // empty scriptSig
    tx.extend_from_slice(&sequence.to_le_bytes());
    tx.push(1);
    tx.extend_from_slice(&0i64.to_le_bytes());
    tx.push(0); // empty scriptPubKey
    compact_size(stack.len(), &mut tx);
    for item in stack {
        compact_size(item.len(), &mut tx);
        tx.extend_from_slice(item);
    }
    tx.extend_from_slice(&locktime.to_le_bytes());
    tx
}

impl Kernel {
    pub fn load(path: &str) -> Result<Self> {
        // SAFETY: loading a library runs its initialisers; the operator names it
        let lib = unsafe { Library::new(path) }.with_context(|| format!("loading {path}"))?;
        let kernel = Kernel { lib };
        // fail at startup, not on the first script spend
        for name in [
            "btck_script_pubkey_create",
            "btck_script_pubkey_destroy",
            "btck_transaction_create",
            "btck_transaction_destroy",
            "btck_transaction_output_create",
            "btck_transaction_output_destroy",
            "btck_precomputed_transaction_data_create",
            "btck_precomputed_transaction_data_destroy",
            "btck_script_pubkey_verify",
        ] {
            // SAFETY: only checks the symbol exists
            unsafe { kernel.lib.get::<*const c_void>(name.as_bytes()) }.with_context(|| {
                format!("{name} missing from {path} - wrong Bitcoin Core version?")
            })?;
        }
        Ok(kernel)
    }

    fn owned<'a>(&'a self, ptr: *mut c_void, destroy: &str, what: &str) -> Result<Owned<'a>> {
        if ptr.is_null() {
            bail!("libbitcoinkernel rejected the {what}");
        }
        // SAFETY: the symbol exists (checked in `load`) and has this signature
        let destroy = unsafe { self.lib.get::<Destroy>(destroy.as_bytes()) }?;
        Ok(Owned { ptr, destroy })
    }

    /// Does `cw1` spend the output `OP_1 <output_key>` as input 0 of the
    /// canonical spend transaction at `domain`? Time claims are the mint's
    /// clock, checked elsewhere.
    pub fn verify_script_path(
        &self,
        output_key: &[u8; 32],
        domain: &str,
        cw1: &Cw1,
    ) -> Result<bool> {
        let mut spk = vec![0x51, 0x20];
        spk.extend_from_slice(output_key);
        let mut stack: Vec<&[u8]> = cw1.witness.iter().map(Vec::as_slice).collect();
        stack.push(&cw1.script);
        stack.push(&cw1.control_block);
        let tx = spend_tx(domain, &stack, cw1.locktime, cw1.sequence);

        // SAFETY: every symbol was checked in `load` and matches Core's
        // bitcoinkernel.h; every handle outlives the calls that use it and
        // is freed once, dependents first (`Owned`s drop in reverse order)
        unsafe {
            let spk_create = self.lib.get::<Create>(b"btck_script_pubkey_create")?;
            let tx_create = self.lib.get::<Create>(b"btck_transaction_create")?;
            let output_create = self
                .lib
                .get::<OutputCreate>(b"btck_transaction_output_create")?;
            let txdata_create = self
                .lib
                .get::<TxDataCreate>(b"btck_precomputed_transaction_data_create")?;
            let verify = self.lib.get::<Verify>(b"btck_script_pubkey_verify")?;

            let script = self.owned(
                spk_create(spk.as_ptr(), spk.len()),
                "btck_script_pubkey_destroy",
                "script",
            )?;
            let tx = self.owned(
                tx_create(tx.as_ptr(), tx.len()),
                "btck_transaction_destroy",
                "transaction",
            )?;
            let spent_script = self.owned(
                spk_create(spk.as_ptr(), spk.len()),
                "btck_script_pubkey_destroy",
                "spent script",
            )?;
            let spent = self.owned(
                output_create(spent_script.ptr, 0),
                "btck_transaction_output_destroy",
                "spent output",
            )?;
            let outputs = [spent.ptr as *const c_void];
            let txdata = self.owned(
                txdata_create(tx.ptr, outputs.as_ptr(), outputs.len()),
                "btck_precomputed_transaction_data_destroy",
                "precomputed transaction data",
            )?;
            let mut status = 0u8;
            let ok = verify(script.ptr, 0, tx.ptr, txdata.ptr, 0, FLAGS_ALL, &mut status);
            if status != 0 {
                bail!("btck_script_pubkey_verify failed to run (status {status})");
            }
            Ok(ok == 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use lnurlcash_core::{
        recoverable::encode_cw1,
        spend::{bearer_cw1, key_path_sighash},
    };
    use sha2::{Digest, Sha256};

    use super::*;

    /// Set LNURLCASHKERNEL_LIB to a libbitcoinkernel build to run these.
    fn kernel() -> Option<Kernel> {
        let path = std::env::var("LNURLCASHKERNEL_LIB").ok()?;
        Some(Kernel::load(&path).expect("loads"))
    }

    #[test]
    fn the_canonical_tx_matches_the_key_path_vector() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/vectors/25-vectors.json")).unwrap();
        let v = &v["key_path_spend"];
        let sig = hex::decode(v["sig"].as_str().unwrap()).unwrap();
        let tx = spend_tx(v["domain"].as_str().unwrap(), &[&sig], 0, 0xffff_ffff);
        assert_eq!(hex::encode(tx), v["spend_tx"].as_str().unwrap());
        let q: [u8; 32] = hex::decode(v["Q"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(
            hex::encode(key_path_sighash(&q, "mint.example")),
            v["sighash"].as_str().unwrap()
        );
    }

    #[test]
    fn core_agrees_on_leaves_core_alone_can_judge() {
        let Some(kernel) = kernel() else { return };
        let preimage = [9u8; 32];
        // the bearer leaf: what lnurlcash-core decides, Core decides too
        let cw1 = bearer_cw1(&preimage).unwrap();
        let q = cw1.output_key().unwrap();
        assert!(kernel.verify_script_path(&q, "mint.example", &cw1).unwrap());
        let mut wrong = cw1.clone();
        wrong.witness = vec![vec![1; 32]];
        assert!(
            !kernel
                .verify_script_path(&q, "mint.example", &wrong)
                .unwrap()
        );

        // a leaf lnurlcash-core leaves unevaluated: hashlock behind a CLTV
        let h = Sha256::digest(preimage);
        let mut script = vec![0x04];
        script.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        script.extend_from_slice(&[0xb1, 0x75, 0xa8, 0x20]); // CLTV DROP SHA256 <32>
        script.extend_from_slice(&h);
        script.push(0x87); // EQUAL
        let leaf = lnurlcash_core::spend::tapleaf_hash(&script, 0xc0);
        let tweak =
            lnurlcash_core::spend::taproot_tweak(&lnurlcash_core::spend::NUMS_H, &leaf).unwrap();
        let mut control = vec![0xc0 | tweak.parity];
        control.extend_from_slice(&lnurlcash_core::spend::NUMS_H);
        let timelocked = Cw1 {
            locktime: 1_700_000_001,
            sequence: 0xffff_fffe,
            script,
            control_block: control,
            witness: vec![preimage.to_vec()],
        };
        assert_eq!(timelocked.output_key(), Some(tweak.output_key));
        assert!(
            kernel
                .verify_script_path(&tweak.output_key, "mint.example", &timelocked)
                .unwrap()
        );
        let early = Cw1 {
            locktime: 1_600_000_000,
            ..timelocked.clone()
        };
        assert!(
            !kernel
                .verify_script_path(&tweak.output_key, "mint.example", &early)
                .unwrap()
        );
        // and the plugin's own spend check reaches the same verdict through it
        let parsed = crate::spend::parse(&encode_cw1(&timelocked).unwrap()).unwrap();
        let domains = ["mint.example".to_string()];
        assert_eq!(
            crate::spend::verify(&parsed, 0, &domains, 1_800_000_000, Some(&kernel)),
            None
        );
        assert!(crate::spend::verify(&parsed, 0, &domains, 1_800_000_000, None).is_some());
    }
}
