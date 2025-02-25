// Copyright 2020 The Matrix.org Foundation C.I.C.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::{Cursor, Read, Seek, SeekFrom};

use aes_ctr::{
    cipher::{NewStreamCipher, SyncStreamCipher},
    Aes256Ctr,
};
use byteorder::{BigEndian, ReadBytesExt};
use getrandom::getrandom;
use hmac::{Hmac, Mac, NewMac};
use pbkdf2::pbkdf2;
use serde_json::Error as SerdeError;
use sha2::{Sha256, Sha512};
use thiserror::Error;

use crate::{
    olm::ExportedRoomKey,
    utilities::{decode, encode, DecodeError},
};

const SALT_SIZE: usize = 16;
const IV_SIZE: usize = 16;
const MAC_SIZE: usize = 32;
const KEY_SIZE: usize = 32;
const VERSION: u8 = 1;

const HEADER: &str = "-----BEGIN MEGOLM SESSION DATA-----";
const FOOTER: &str = "-----END MEGOLM SESSION DATA-----";

/// Error representing a failure during key export or import.
#[derive(Error, Debug)]
pub enum KeyExportError {
    /// The key export doesn't contain valid headers.
    #[error("Invalid or missing key export headers.")]
    InvalidHeaders,
    /// The key export has been encrypted with an unsupported version.
    #[error("The key export has been encrypted with an unsupported version.")]
    UnsupportedVersion,
    /// The MAC of the encrypted payload is invalid.
    #[error("The MAC of the encrypted payload is invalid.")]
    InvalidMac,
    /// The decrypted key export isn't valid UTF-8.
    #[error(transparent)]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    /// The decrypted key export doesn't contain valid JSON.
    #[error(transparent)]
    Json(#[from] SerdeError),
    /// The key export string isn't valid base64.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The key export doesn't all the required fields.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Try to decrypt a reader into a list of exported room keys.
///
/// # Arguments
///
/// * `passphrase` - The passphrase that was used to encrypt the exported keys.
///
/// # Examples
/// ```no_run
/// # use std::io::Cursor;
/// # use matrix_sdk_crypto::{OlmMachine, decrypt_key_export};
/// # use ruma::user_id;
/// # use futures::executor::block_on;
/// # let alice = user_id!("@alice:example.org");
/// # let machine = OlmMachine::new(&alice, "DEVICEID".into());
/// # block_on(async {
/// # let export = Cursor::new("".to_owned());
/// let exported_keys = decrypt_key_export(export, "1234").unwrap();
/// machine.import_keys(exported_keys, |_, _| {}).await.unwrap();
/// # });
/// ```
pub fn decrypt_key_export(
    mut input: impl Read,
    passphrase: &str,
) -> Result<Vec<ExportedRoomKey>, KeyExportError> {
    let mut x: String = String::new();

    input.read_to_string(&mut x)?;

    if !(x.trim_start().starts_with(HEADER) && x.trim_end().ends_with(FOOTER)) {
        return Err(KeyExportError::InvalidHeaders);
    }

    let payload: String =
        x.lines().filter(|l| !(l.starts_with(HEADER) || l.starts_with(FOOTER))).collect();

    Ok(serde_json::from_str(&decrypt_helper(&payload, passphrase)?)?)
}

/// Encrypt the list of exported room keys using the given passphrase.
///
/// # Arguments
///
/// * `keys` - A list of sessions that should be encrypted.
///
/// * `passphrase` - The passphrase that will be used to encrypt the exported
/// room keys.
///
/// * `rounds` - The number of rounds that should be used for the key
/// derivation when the passphrase gets turned into an AES key. More rounds are
/// increasingly computationally intensive and as such help against brute-force
/// attacks. Should be at least `10000`, while values in the `100000` ranges
/// should be preferred.
///
/// # Panics
///
/// This method will panic if it can't get enough randomness from the OS to
/// encrypt the exported keys securely.
///
/// # Examples
/// ```no_run
/// # use matrix_sdk_crypto::{OlmMachine, encrypt_key_export};
/// # use ruma::{user_id, room_id};
/// # use futures::executor::block_on;
/// # let alice = user_id!("@alice:example.org");
/// # let machine = OlmMachine::new(&alice, "DEVICEID".into());
/// # block_on(async {
/// let room_id = room_id!("!test:localhost");
/// let exported_keys = machine.export_keys(|s| s.room_id() == &room_id).await.unwrap();
/// let encrypted_export = encrypt_key_export(&exported_keys, "1234", 1);
/// # });
/// ```
pub fn encrypt_key_export(
    keys: &[ExportedRoomKey],
    passphrase: &str,
    rounds: u32,
) -> Result<String, SerdeError> {
    let mut plaintext = serde_json::to_string(keys)?.into_bytes();
    let ciphertext = encrypt_helper(&mut plaintext, passphrase, rounds);
    Ok([HEADER.to_owned(), ciphertext, FOOTER.to_owned()].join("\n"))
}

fn encrypt_helper(mut plaintext: &mut [u8], passphrase: &str, rounds: u32) -> String {
    let mut salt = [0u8; SALT_SIZE];
    let mut iv = [0u8; IV_SIZE];
    let mut derived_keys = [0u8; KEY_SIZE * 2];

    getrandom(&mut salt).expect("Can't generate randomness");
    getrandom(&mut iv).expect("Can't generate randomness");

    let mut iv = u128::from_be_bytes(iv);
    iv &= !(1 << 63);

    pbkdf2::<Hmac<Sha512>>(passphrase.as_bytes(), &salt, rounds, &mut derived_keys);
    let (key, hmac_key) = derived_keys.split_at(KEY_SIZE);

    let mut aes = Aes256Ctr::new_var(key, &iv.to_be_bytes()).expect("Can't create AES object");

    aes.apply_keystream(&mut plaintext);

    let mut payload: Vec<u8> = vec![];

    payload.extend(&VERSION.to_be_bytes());
    payload.extend(&salt);
    payload.extend(&iv.to_be_bytes());
    payload.extend(&rounds.to_be_bytes());
    payload.extend_from_slice(plaintext);

    let mut hmac = Hmac::<Sha256>::new_varkey(hmac_key).expect("Can't create HMAC object");
    hmac.update(&payload);
    let mac = hmac.finalize();

    payload.extend(mac.into_bytes());

    encode(payload)
}

fn decrypt_helper(ciphertext: &str, passphrase: &str) -> Result<String, KeyExportError> {
    let decoded = decode(ciphertext)?;

    let mut decoded = Cursor::new(decoded);

    let mut salt = [0u8; SALT_SIZE];
    let mut iv = [0u8; IV_SIZE];
    let mut mac = [0u8; MAC_SIZE];
    let mut derived_keys = [0u8; KEY_SIZE * 2];

    let version = decoded.read_u8()?;
    decoded.read_exact(&mut salt)?;
    decoded.read_exact(&mut iv)?;

    let rounds = decoded.read_u32::<BigEndian>()?;
    let ciphertext_start = decoded.position() as usize;

    decoded.seek(SeekFrom::End(-32))?;
    let ciphertext_end = decoded.position() as usize;

    decoded.read_exact(&mut mac)?;

    let mut decoded = decoded.into_inner();

    if version != VERSION {
        return Err(KeyExportError::UnsupportedVersion);
    }

    pbkdf2::<Hmac<Sha512>>(passphrase.as_bytes(), &salt, rounds, &mut derived_keys);
    let (key, hmac_key) = derived_keys.split_at(KEY_SIZE);

    let mut hmac = Hmac::<Sha256>::new_varkey(hmac_key).expect("Can't create an HMAC object");
    hmac.update(&decoded[0..ciphertext_end]);
    hmac.verify(&mac).map_err(|_| KeyExportError::InvalidMac)?;

    let mut ciphertext = &mut decoded[ciphertext_start..ciphertext_end];
    let mut aes = Aes256Ctr::new_var(key, &iv).expect("Can't create an AES object");
    aes.apply_keystream(&mut ciphertext);

    Ok(String::from_utf8(ciphertext.to_owned())?)
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use indoc::indoc;
    use matrix_sdk_test::async_test;
    use proptest::prelude::*;
    use ruma::room_id;

    use super::{decode, decrypt_helper, decrypt_key_export, encrypt_helper, encrypt_key_export};
    use crate::machine::test::get_prepared_machine;

    const PASSPHRASE: &str = "1234";

    const TEST_EXPORT: &str = indoc! {"
        -----BEGIN MEGOLM SESSION DATA-----
        Af7mGhlzQ+eGvHu93u0YXd3D/+vYMs3E7gQqOhuCtkvGAAAAASH7pEdWvFyAP1JUisAcpEo
        Xke2Q7Kr9hVl/SCc6jXBNeJCZcrUbUV4D/tRQIl3E9L4fOk928YI1J+3z96qiH0uE7hpsCI
        CkHKwjPU+0XTzFdIk1X8H7sZ+MD/2Sg/q3y8rtUjz7uEj4GUTnb+9SCOTVmJsRfqgUpM1CU
        bDLytHf1JkohY4tWEgpsCc67xdzgodjr12qYrfg/zNm3LGpxlrffJknw4rk5QFTj4kMbqbD
        ZZgDTni+HxRTDGge2J620lMOiznvXX+H09Rwruqx5aJvvaaKd86jWRpiO2oSFqHn4u5ONl9
        41uzm62Sj0eIm6ZbA9NQs87jQw4LxsejhZVL+NdjIg80zVSBTWhTdo0DTnbFSNP4ReOiz0U
        XosOF8A5T8Vdx2nvA0GXltfcHKVKQYh/LJAkNQ7P9UYL4ae/5TtQZkhB1KxCLTRWqADCl53
        uBMGpG53EMgY6G6K2DEIOkcv7sdXQF5WpemiSWZqJRWj+cjfs9BpCTbkp/rszWFl2TniWpR
        RqIbT2jORlN4rTvdtF0F4z1pqP4qWyR3sLNTkXm9CFRzWADNG0RDZKxbCoo6RPvtaCTfaHo
        SwfvzBS6CjfAG+FOugpV48o7+XetaUUPZ6/tZSPhCdeV8eP9q5r0QwWeXFogzoNzWt4HYx9
        MdXxzD+f0mtg5gzehrrEEARwI2bCvPpHxlt/Na9oW/GBpkjwR1LSKgg4CtpRyWngPjdEKpZ
        GYW19pdjg0qdXNk/eqZsQTsNWVo6A
        -----END MEGOLM SESSION DATA-----
    "};

    fn export_wihtout_headers() -> String {
        TEST_EXPORT.lines().filter(|l| !l.starts_with("-----")).collect()
    }

    #[test]
    fn test_decode() {
        let export = export_wihtout_headers();
        assert!(decode(export).is_ok());
    }

    proptest! {
        #[test]
        fn proptest_encrypt_cycle(plaintext in prop::string::string_regex(".*").unwrap()) {
            let mut plaintext_bytes = plaintext.clone().into_bytes();

            let ciphertext = encrypt_helper(&mut plaintext_bytes, "test", 1);
            let decrypted = decrypt_helper(&ciphertext, "test").unwrap();

            prop_assert!(plaintext == decrypted);
        }
    }

    #[test]
    fn test_encrypt_decrypt() {
        let data = "It's a secret to everybody";
        let mut bytes = data.to_owned().into_bytes();

        let encrypted = encrypt_helper(&mut bytes, PASSPHRASE, 10);
        let decrypted = decrypt_helper(&encrypted, PASSPHRASE).unwrap();

        assert_eq!(data, decrypted);
    }

    #[async_test]
    async fn test_session_encrypt() {
        let (machine, _) = get_prepared_machine().await;
        let room_id = room_id!("!test:localhost");

        machine.create_outbound_group_session_with_defaults(&room_id).await.unwrap();
        let export = machine.export_keys(|s| s.room_id() == &room_id).await.unwrap();

        assert!(!export.is_empty());

        let encrypted = encrypt_key_export(&export, "1234", 1).unwrap();
        let decrypted = decrypt_key_export(Cursor::new(encrypted), "1234").unwrap();

        assert_eq!(export, decrypted);
        assert_eq!(machine.import_keys(decrypted, |_, _| {}).await.unwrap(), (0, 1));
    }

    #[test]
    fn test_real_decrypt() {
        let reader = Cursor::new(TEST_EXPORT);
        let imported = decrypt_key_export(reader, PASSPHRASE).expect("Can't decrypt key export");
        assert!(!imported.is_empty())
    }
}
