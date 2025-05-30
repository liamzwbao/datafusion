// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Encoding expressions

use arrow::{
    array::{
        Array, ArrayRef, BinaryArray, GenericByteArray, OffsetSizeTrait, StringArray,
    },
    datatypes::{ByteArrayType, DataType},
};
use arrow_buffer::{Buffer, OffsetBufferBuilder};
use base64::{engine::general_purpose, Engine as _};
use datafusion_common::{
    cast::{as_generic_binary_array, as_generic_string_array},
    not_impl_err, plan_err,
    utils::take_function_args,
};
use datafusion_common::{exec_err, ScalarValue};
use datafusion_common::{DataFusionError, Result};
use datafusion_expr::{ColumnarValue, Documentation};
use std::sync::Arc;
use std::{fmt, str::FromStr};

use datafusion_expr::{ScalarUDFImpl, Signature, Volatility};
use datafusion_macros::user_doc;
use std::any::Any;

#[user_doc(
    doc_section(label = "Binary String Functions"),
    description = "Encode binary data into a textual representation.",
    syntax_example = "encode(expression, format)",
    argument(
        name = "expression",
        description = "Expression containing string or binary data"
    ),
    argument(
        name = "format",
        description = "Supported formats are: `base64`, `hex`"
    ),
    related_udf(name = "decode")
)]
#[derive(Debug)]
pub struct EncodeFunc {
    signature: Signature,
}

impl Default for EncodeFunc {
    fn default() -> Self {
        Self::new()
    }
}

impl EncodeFunc {
    pub fn new() -> Self {
        Self {
            signature: Signature::user_defined(Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for EncodeFunc {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn name(&self) -> &str {
        "encode"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        use DataType::*;

        Ok(match arg_types[0] {
            Utf8 => Utf8,
            LargeUtf8 => LargeUtf8,
            Utf8View => Utf8,
            Binary => Utf8,
            LargeBinary => LargeUtf8,
            Null => Null,
            _ => {
                return plan_err!(
                    "The encode function can only accept Utf8 or Binary or Null."
                );
            }
        })
    }

    fn invoke_with_args(
        &self,
        args: datafusion_expr::ScalarFunctionArgs,
    ) -> Result<ColumnarValue> {
        encode(&args.args)
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        let [expression, format] = take_function_args(self.name(), arg_types)?;

        if format != &DataType::Utf8 {
            return Err(DataFusionError::Plan("2nd argument should be Utf8".into()));
        }

        match expression {
            DataType::Utf8 | DataType::Utf8View | DataType::Null => {
                Ok(vec![DataType::Utf8; 2])
            }
            DataType::LargeUtf8 => Ok(vec![DataType::LargeUtf8, DataType::Utf8]),
            DataType::Binary => Ok(vec![DataType::Binary, DataType::Utf8]),
            DataType::LargeBinary => Ok(vec![DataType::LargeBinary, DataType::Utf8]),
            _ => plan_err!(
                "1st argument should be Utf8 or Binary or Null, got {:?}",
                arg_types[0]
            ),
        }
    }

    fn documentation(&self) -> Option<&Documentation> {
        self.doc()
    }
}

#[user_doc(
    doc_section(label = "Binary String Functions"),
    description = "Decode binary data from textual representation in string.",
    syntax_example = "decode(expression, format)",
    argument(
        name = "expression",
        description = "Expression containing encoded string data"
    ),
    argument(name = "format", description = "Same arguments as [encode](#encode)"),
    related_udf(name = "encode")
)]
#[derive(Debug)]
pub struct DecodeFunc {
    signature: Signature,
}

impl Default for DecodeFunc {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodeFunc {
    pub fn new() -> Self {
        Self {
            signature: Signature::user_defined(Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for DecodeFunc {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn name(&self) -> &str {
        "decode"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        Ok(arg_types[0].to_owned())
    }

    fn invoke_with_args(
        &self,
        args: datafusion_expr::ScalarFunctionArgs,
    ) -> Result<ColumnarValue> {
        decode(&args.args)
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        if arg_types.len() != 2 {
            return plan_err!(
                "{} expects to get 2 arguments, but got {}",
                self.name(),
                arg_types.len()
            );
        }

        if arg_types[1] != DataType::Utf8 {
            return plan_err!("2nd argument should be Utf8");
        }

        match arg_types[0] {
            DataType::Utf8 | DataType::Utf8View | DataType::Binary | DataType::Null => {
                Ok(vec![DataType::Binary, DataType::Utf8])
            }
            DataType::LargeUtf8 | DataType::LargeBinary => {
                Ok(vec![DataType::LargeBinary, DataType::Utf8])
            }
            _ => plan_err!(
                "1st argument should be Utf8 or Binary or Null, got {:?}",
                arg_types[0]
            ),
        }
    }

    fn documentation(&self) -> Option<&Documentation> {
        self.doc()
    }
}

#[derive(Debug, Copy, Clone)]
enum Encoding {
    Base64,
    Hex,
}

fn encode_process(value: &ColumnarValue, encoding: Encoding) -> Result<ColumnarValue> {
    match value {
        ColumnarValue::Array(a) => match a.data_type() {
            DataType::Utf8 => encoding.encode_utf8_array::<i32>(a.as_ref()),
            DataType::LargeUtf8 => encoding.encode_utf8_array::<i64>(a.as_ref()),
            DataType::Utf8View => encoding.encode_utf8_array::<i32>(a.as_ref()),
            DataType::Binary => encoding.encode_binary_array::<i32>(a.as_ref()),
            DataType::LargeBinary => encoding.encode_binary_array::<i64>(a.as_ref()),
            other => exec_err!(
                "Unsupported data type {other:?} for function encode({encoding})"
            ),
        },
        ColumnarValue::Scalar(scalar) => {
            match scalar {
                ScalarValue::Utf8(a) => {
                    Ok(encoding.encode_scalar(a.as_ref().map(|s: &String| s.as_bytes())))
                }
                ScalarValue::LargeUtf8(a) => Ok(encoding
                    .encode_large_scalar(a.as_ref().map(|s: &String| s.as_bytes()))),
                ScalarValue::Utf8View(a) => {
                    Ok(encoding.encode_scalar(a.as_ref().map(|s: &String| s.as_bytes())))
                }
                ScalarValue::Binary(a) => Ok(
                    encoding.encode_scalar(a.as_ref().map(|v: &Vec<u8>| v.as_slice()))
                ),
                ScalarValue::LargeBinary(a) => Ok(encoding
                    .encode_large_scalar(a.as_ref().map(|v: &Vec<u8>| v.as_slice()))),
                other => exec_err!(
                    "Unsupported data type {other:?} for function encode({encoding})"
                ),
            }
        }
    }
}

fn decode_process(value: &ColumnarValue, encoding: Encoding) -> Result<ColumnarValue> {
    match value {
        ColumnarValue::Array(a) => match a.data_type() {
            DataType::Utf8 => encoding.decode_utf8_array::<i32>(a.as_ref()),
            DataType::LargeUtf8 => encoding.decode_utf8_array::<i64>(a.as_ref()),
            DataType::Utf8View => encoding.decode_utf8_array::<i32>(a.as_ref()),
            DataType::Binary => encoding.decode_binary_array::<i32>(a.as_ref()),
            DataType::LargeBinary => encoding.decode_binary_array::<i64>(a.as_ref()),
            other => exec_err!(
                "Unsupported data type {other:?} for function decode({encoding})"
            ),
        },
        ColumnarValue::Scalar(scalar) => {
            match scalar {
                ScalarValue::Utf8(a) => {
                    encoding.decode_scalar(a.as_ref().map(|s: &String| s.as_bytes()))
                }
                ScalarValue::LargeUtf8(a) => encoding
                    .decode_large_scalar(a.as_ref().map(|s: &String| s.as_bytes())),
                ScalarValue::Utf8View(a) => {
                    encoding.decode_scalar(a.as_ref().map(|s: &String| s.as_bytes()))
                }
                ScalarValue::Binary(a) => {
                    encoding.decode_scalar(a.as_ref().map(|v: &Vec<u8>| v.as_slice()))
                }
                ScalarValue::LargeBinary(a) => encoding
                    .decode_large_scalar(a.as_ref().map(|v: &Vec<u8>| v.as_slice())),
                other => exec_err!(
                    "Unsupported data type {other:?} for function decode({encoding})"
                ),
            }
        }
    }
}

fn hex_encode(input: &[u8]) -> String {
    hex::encode(input)
}

fn base64_encode(input: &[u8]) -> String {
    general_purpose::STANDARD_NO_PAD.encode(input)
}

fn hex_decode(input: &[u8], buf: &mut [u8]) -> Result<usize> {
    // only write input / 2 bytes to buf
    let out_len = input.len() / 2;
    let buf = &mut buf[..out_len];
    hex::decode_to_slice(input, buf).map_err(|e| {
        DataFusionError::Internal(format!("Failed to decode from hex: {e}"))
    })?;
    Ok(out_len)
}

fn base64_decode(input: &[u8], buf: &mut [u8]) -> Result<usize> {
    general_purpose::STANDARD_NO_PAD
        .decode_slice(input, buf)
        .map_err(|e| {
            DataFusionError::Internal(format!("Failed to decode from base64: {e}"))
        })
}

macro_rules! encode_to_array {
    ($METHOD: ident, $INPUT:expr) => {{
        let utf8_array: StringArray = $INPUT
            .iter()
            .map(|x| x.map(|x| $METHOD(x.as_ref())))
            .collect();
        Arc::new(utf8_array)
    }};
}

fn decode_to_array<F, T: ByteArrayType>(
    method: F,
    input: &GenericByteArray<T>,
    conservative_upper_bound_size: usize,
) -> Result<ArrayRef>
where
    F: Fn(&[u8], &mut [u8]) -> Result<usize>,
{
    let mut values = vec![0; conservative_upper_bound_size];
    let mut offsets = OffsetBufferBuilder::new(input.len());
    let mut total_bytes_decoded = 0;
    for v in input {
        if let Some(v) = v {
            let cursor = &mut values[total_bytes_decoded..];
            let decoded = method(v.as_ref(), cursor)?;
            total_bytes_decoded += decoded;
            offsets.push_length(decoded);
        } else {
            offsets.push_length(0);
        }
    }
    // We reserved an upper bound size for the values buffer, but we only use the actual size
    values.truncate(total_bytes_decoded);
    let binary_array = BinaryArray::try_new(
        offsets.finish(),
        Buffer::from_vec(values),
        input.nulls().cloned(),
    )?;
    Ok(Arc::new(binary_array))
}

impl Encoding {
    fn encode_scalar(self, value: Option<&[u8]>) -> ColumnarValue {
        ColumnarValue::Scalar(match self {
            Self::Base64 => ScalarValue::Utf8(
                value.map(|v| general_purpose::STANDARD_NO_PAD.encode(v)),
            ),
            Self::Hex => ScalarValue::Utf8(value.map(hex::encode)),
        })
    }

    fn encode_large_scalar(self, value: Option<&[u8]>) -> ColumnarValue {
        ColumnarValue::Scalar(match self {
            Self::Base64 => ScalarValue::LargeUtf8(
                value.map(|v| general_purpose::STANDARD_NO_PAD.encode(v)),
            ),
            Self::Hex => ScalarValue::LargeUtf8(value.map(hex::encode)),
        })
    }

    fn encode_binary_array<T>(self, value: &dyn Array) -> Result<ColumnarValue>
    where
        T: OffsetSizeTrait,
    {
        let input_value = as_generic_binary_array::<T>(value)?;
        let array: ArrayRef = match self {
            Self::Base64 => encode_to_array!(base64_encode, input_value),
            Self::Hex => encode_to_array!(hex_encode, input_value),
        };
        Ok(ColumnarValue::Array(array))
    }

    fn encode_utf8_array<T>(self, value: &dyn Array) -> Result<ColumnarValue>
    where
        T: OffsetSizeTrait,
    {
        let input_value = as_generic_string_array::<T>(value)?;
        let array: ArrayRef = match self {
            Self::Base64 => encode_to_array!(base64_encode, input_value),
            Self::Hex => encode_to_array!(hex_encode, input_value),
        };
        Ok(ColumnarValue::Array(array))
    }

    fn decode_scalar(self, value: Option<&[u8]>) -> Result<ColumnarValue> {
        let value = match value {
            Some(value) => value,
            None => return Ok(ColumnarValue::Scalar(ScalarValue::Binary(None))),
        };

        let out = match self {
            Self::Base64 => {
                general_purpose::STANDARD_NO_PAD
                    .decode(value)
                    .map_err(|e| {
                        DataFusionError::Internal(format!(
                            "Failed to decode value using base64: {e}"
                        ))
                    })?
            }
            Self::Hex => hex::decode(value).map_err(|e| {
                DataFusionError::Internal(format!(
                    "Failed to decode value using hex: {e}"
                ))
            })?,
        };

        Ok(ColumnarValue::Scalar(ScalarValue::Binary(Some(out))))
    }

    fn decode_large_scalar(self, value: Option<&[u8]>) -> Result<ColumnarValue> {
        let value = match value {
            Some(value) => value,
            None => return Ok(ColumnarValue::Scalar(ScalarValue::LargeBinary(None))),
        };

        let out = match self {
            Self::Base64 => {
                general_purpose::STANDARD_NO_PAD
                    .decode(value)
                    .map_err(|e| {
                        DataFusionError::Internal(format!(
                            "Failed to decode value using base64: {e}"
                        ))
                    })?
            }
            Self::Hex => hex::decode(value).map_err(|e| {
                DataFusionError::Internal(format!(
                    "Failed to decode value using hex: {e}"
                ))
            })?,
        };

        Ok(ColumnarValue::Scalar(ScalarValue::LargeBinary(Some(out))))
    }

    fn decode_binary_array<T>(self, value: &dyn Array) -> Result<ColumnarValue>
    where
        T: OffsetSizeTrait,
    {
        let input_value = as_generic_binary_array::<T>(value)?;
        let array = self.decode_byte_array(input_value)?;
        Ok(ColumnarValue::Array(array))
    }

    fn decode_utf8_array<T>(self, value: &dyn Array) -> Result<ColumnarValue>
    where
        T: OffsetSizeTrait,
    {
        let input_value = as_generic_string_array::<T>(value)?;
        let array = self.decode_byte_array(input_value)?;
        Ok(ColumnarValue::Array(array))
    }

    fn decode_byte_array<T: ByteArrayType>(
        &self,
        input_value: &GenericByteArray<T>,
    ) -> Result<ArrayRef> {
        match self {
            Self::Base64 => {
                let upper_bound =
                    base64::decoded_len_estimate(input_value.values().len());
                decode_to_array(base64_decode, input_value, upper_bound)
            }
            Self::Hex => {
                // Calculate the upper bound for decoded byte size
                // For hex encoding, each pair of hex characters (2 bytes) represents 1 byte when decoded
                // So the upper bound is half the length of the input values.
                let upper_bound = input_value.values().len() / 2;
                decode_to_array(hex_decode, input_value, upper_bound)
            }
        }
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", format!("{self:?}").to_lowercase())
    }
}

impl FromStr for Encoding {
    type Err = DataFusionError;
    fn from_str(name: &str) -> Result<Encoding> {
        Ok(match name {
            "base64" => Self::Base64,
            "hex" => Self::Hex,
            _ => {
                let options = [Self::Base64, Self::Hex]
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return plan_err!(
                    "There is no built-in encoding named '{name}', currently supported encodings are: {options}"
                );
            }
        })
    }
}

/// Encodes the given data, accepts Binary, LargeBinary, Utf8, Utf8View or LargeUtf8 and returns a [`ColumnarValue`].
/// Second argument is the encoding to use.
/// Standard encodings are base64 and hex.
fn encode(args: &[ColumnarValue]) -> Result<ColumnarValue> {
    let [expression, format] = take_function_args("encode", args)?;

    let encoding = match format {
        ColumnarValue::Scalar(scalar) => match scalar.try_as_str() {
            Some(Some(method)) => method.parse::<Encoding>(),
            _ => not_impl_err!(
                "Second argument to encode must be non null constant string: Encode using dynamically decided method is not yet supported. Got {scalar:?}"
            ),
        },
        ColumnarValue::Array(_) => not_impl_err!(
            "Second argument to encode must be a constant: Encode using dynamically decided method is not yet supported"
        ),
    }?;
    encode_process(expression, encoding)
}

/// Decodes the given data, accepts Binary, LargeBinary, Utf8, Utf8View or LargeUtf8 and returns a [`ColumnarValue`].
/// Second argument is the encoding to use.
/// Standard encodings are base64 and hex.
fn decode(args: &[ColumnarValue]) -> Result<ColumnarValue> {
    let [expression, format] = take_function_args("decode", args)?;

    let encoding = match format {
        ColumnarValue::Scalar(scalar) => match scalar.try_as_str() {
            Some(Some(method))=> method.parse::<Encoding>(),
            _ => not_impl_err!(
                "Second argument to decode must be a non null constant string: Decode using dynamically decided method is not yet supported. Got {scalar:?}"
            ),
        },
        ColumnarValue::Array(_) => not_impl_err!(
            "Second argument to decode must be a utf8 constant: Decode using dynamically decided method is not yet supported"
        ),
    }?;
    decode_process(expression, encoding)
}
