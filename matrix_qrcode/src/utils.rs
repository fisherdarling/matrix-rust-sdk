// Copyright 2021 The Matrix.org Foundation C.I.C.
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

use std::convert::TryInto;

use base64::{decode_config, encode_config, STANDARD_NO_PAD};
#[cfg(feature = "decode_image")]
use image::{ImageBuffer, Luma};
use qrcode::QrCode;

#[cfg(feature = "decode_image")]
use crate::error::DecodingError;
use crate::error::EncodingError;

pub(crate) const HEADER: &[u8] = b"MATRIX";
pub(crate) const VERSION: u8 = 0x2;
pub(crate) const MAX_MODE: u8 = 0x2;
pub(crate) const MIN_SECRET_LEN: usize = 8;

pub(crate) fn base_64_encode(data: &[u8]) -> String {
    encode_config(data, STANDARD_NO_PAD)
}

pub(crate) fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    decode_config(data, STANDARD_NO_PAD)
}

pub(crate) fn to_bytes(
    mode: u8,
    flow_id: &str,
    first_key: &str,
    second_key: &str,
    shared_secret: &str,
) -> Result<Vec<u8>, EncodingError> {
    let flow_id_len: u16 = flow_id.len().try_into()?;
    let flow_id_len = flow_id_len.to_be_bytes();

    let first_key = base64_decode(first_key)?;
    let second_key = base64_decode(second_key)?;
    let shared_secret = base64_decode(shared_secret)?;

    let data = [
        HEADER,
        &[VERSION],
        &[mode],
        flow_id_len.as_ref(),
        flow_id.as_bytes(),
        &first_key,
        &second_key,
        &shared_secret,
    ]
    .concat();

    Ok(data)
}

pub(crate) fn to_qr_code(
    mode: u8,
    flow_id: &str,
    first_key: &str,
    second_key: &str,
    shared_secret: &str,
) -> Result<QrCode, EncodingError> {
    let data = to_bytes(mode, flow_id, first_key, second_key, shared_secret)?;
    Ok(QrCode::new(data)?)
}

#[cfg(feature = "decode_image")]
pub(crate) fn decode_qr(image: ImageBuffer<Luma<u8>, Vec<u8>>) -> Result<Vec<u8>, DecodingError> {
    let mut image = rqrr::PreparedImage::prepare(image);
    let grids = image.detect_grids();

    let mut error = None;

    for grid in grids {
        let mut decoded = Vec::new();

        match grid.decode_to(&mut decoded) {
            Ok(_) => {
                if decoded.starts_with(HEADER) {
                    return Ok(decoded);
                }
            }
            Err(e) => error = Some(e),
        }
    }

    Err(error.map(|e| e.into()).unwrap_or_else(|| DecodingError::Header))
}
