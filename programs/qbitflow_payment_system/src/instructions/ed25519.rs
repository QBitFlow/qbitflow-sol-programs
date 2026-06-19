// Solana doesn't let on-chain programs run Ed25519 verification directly. The standard
// pattern is to push that work into the `Ed25519Program` precompile: the caller prepends
// an Ed25519Program instruction in the same transaction that carries `(pubkey, message,
// signature)`; the runtime verifies the signature before our handler even runs. Our handler
// then reads the Instructions sysvar and confirms that the precompile's pubkey + message
// match what we expect.
//
// Reference: Ed25519Program instruction layout (per the Solana runtime):
//
//   byte 0      : number of signatures (`count`)
//   byte 1      : padding (must be 0)
//   bytes 2..16 : `Ed25519SignatureOffsets` for signature 0 (14 bytes)
//                   u16 signature_offset
//                   u16 signature_instruction_index   (0xFFFF = this ix)
//                   u16 public_key_offset
//                   u16 public_key_instruction_index  (0xFFFF = this ix)
//                   u16 message_data_offset
//                   u16 message_data_size
//                   u16 message_instruction_index     (0xFFFF = this ix)
//   ...        : repeats `count - 1` more times for additional signatures
//   ...        : the public keys, signatures, and messages, packed at the declared offsets.
//
// We require:
//   - `count == 1` (one signature per precompile ix; reject multi-sig precompiles for clarity)
//   - All three `instruction_index` fields equal 0xFFFF — meaning the pubkey, sig, and
//     message all live in *this* Ed25519 ix's data. This forbids the precompile from
//     pointing at *our* instruction's data (which an attacker could manipulate via
//     the input bytes we already carry as args).

use anchor_lang::prelude::*;
use solana_sdk_ids::ed25519_program;
use solana_instructions_sysvar::{load_instruction_at_checked, ID as INSTRUCTIONS_ID};

use crate::errors::*;

const ED25519_HEADER_LEN: usize = 2;
const ED25519_OFFSETS_LEN: usize = 14;
const ED25519_PUBKEY_LEN: usize = 32;
const ED25519_SIGNATURE_LEN: usize = 64;
const SAME_INSTRUCTION_INDEX: u16 = 0xFFFF;

fn read_u16_le(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or(QBitFlowError::InvalidEd25519Instruction)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Verify that the transaction includes (immediately accessible via the Instructions sysvar
/// at `ix_index`) an Ed25519Program precompile that the runtime already verified, and that
/// the precompile is for `expected_signer` over `expected_message`.
pub fn verify_ed25519_signature(
    instructions_sysvar: &AccountInfo,
    ix_index: u16,
    expected_signer: &Pubkey,
    expected_message: &[u8],
) -> Result<()> {
    // Defense in depth: confirm the account actually is the Instructions sysvar.
    if instructions_sysvar.key() != INSTRUCTIONS_ID {
        return err!(QBitFlowError::MissingEd25519Instruction);
    }

    let ix = load_instruction_at_checked(ix_index as usize, instructions_sysvar)
        .map_err(|_| QBitFlowError::MissingEd25519Instruction)?;

    if ix.program_id != ed25519_program::ID {
        return err!(QBitFlowError::MissingEd25519Instruction);
    }

    let data = ix.data;
    if data.len() < ED25519_HEADER_LEN + ED25519_OFFSETS_LEN {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    let count = data[0];
    if count != 1 {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    let offsets_start = ED25519_HEADER_LEN;
    let signature_offset = read_u16_le(&data, offsets_start)? as usize;
    let signature_ix_index = read_u16_le(&data, offsets_start + 2)?;
    let pubkey_offset = read_u16_le(&data, offsets_start + 4)? as usize;
    let pubkey_ix_index = read_u16_le(&data, offsets_start + 6)?;
    let message_offset = read_u16_le(&data, offsets_start + 8)? as usize;
    let message_size = read_u16_le(&data, offsets_start + 10)? as usize;
    let message_ix_index = read_u16_le(&data, offsets_start + 12)?;

    // The pubkey / sig / message must all be in the Ed25519 ix's own data.
    if signature_ix_index != SAME_INSTRUCTION_INDEX
        || pubkey_ix_index != SAME_INSTRUCTION_INDEX
        || message_ix_index != SAME_INSTRUCTION_INDEX
    {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    // Bounds checks. The signature itself we don't read here (the runtime already verified
    // it), but we want to make sure the layout is sane so we don't read past the buffer.
    let pubkey_end = pubkey_offset
        .checked_add(ED25519_PUBKEY_LEN)
        .ok_or(QBitFlowError::InvalidEd25519Instruction)?;
    let signature_end = signature_offset
        .checked_add(ED25519_SIGNATURE_LEN)
        .ok_or(QBitFlowError::InvalidEd25519Instruction)?;
    let message_end = message_offset
        .checked_add(message_size)
        .ok_or(QBitFlowError::InvalidEd25519Instruction)?;
    if pubkey_end > data.len() || signature_end > data.len() || message_end > data.len() {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    let pubkey_bytes = &data[pubkey_offset..pubkey_end];
    if pubkey_bytes != expected_signer.as_ref() {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    let message_bytes = &data[message_offset..message_end];
    if message_bytes != expected_message {
        return err!(QBitFlowError::InvalidEd25519Instruction);
    }

    Ok(())
}
