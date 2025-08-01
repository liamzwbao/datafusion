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

//! Functionality used both on logical and physical plans

#[cfg(not(feature = "force_hash_collisions"))]
use std::sync::Arc;

use ahash::RandomState;
use arrow::array::types::{IntervalDayTime, IntervalMonthDayNano};
use arrow::array::*;
use arrow::datatypes::*;
#[cfg(not(feature = "force_hash_collisions"))]
use arrow::{downcast_dictionary_array, downcast_primitive_array};

#[cfg(not(feature = "force_hash_collisions"))]
use crate::cast::{
    as_binary_view_array, as_boolean_array, as_fixed_size_list_array,
    as_generic_binary_array, as_large_list_array, as_list_array, as_map_array,
    as_string_array, as_string_view_array, as_struct_array,
};
use crate::error::Result;
#[cfg(not(feature = "force_hash_collisions"))]
use crate::error::_internal_err;

// Combines two hashes into one hash
#[inline]
pub fn combine_hashes(l: u64, r: u64) -> u64 {
    let hash = (17 * 37u64).wrapping_add(l);
    hash.wrapping_mul(37).wrapping_add(r)
}

#[cfg(not(feature = "force_hash_collisions"))]
fn hash_null(random_state: &RandomState, hashes_buffer: &'_ mut [u64], mul_col: bool) {
    if mul_col {
        hashes_buffer.iter_mut().for_each(|hash| {
            // stable hash for null value
            *hash = combine_hashes(random_state.hash_one(1), *hash);
        })
    } else {
        hashes_buffer.iter_mut().for_each(|hash| {
            *hash = random_state.hash_one(1);
        })
    }
}

pub trait HashValue {
    fn hash_one(&self, state: &RandomState) -> u64;
}

impl<T: HashValue + ?Sized> HashValue for &T {
    fn hash_one(&self, state: &RandomState) -> u64 {
        T::hash_one(self, state)
    }
}

macro_rules! hash_value {
    ($($t:ty),+) => {
        $(impl HashValue for $t {
            fn hash_one(&self, state: &RandomState) -> u64 {
                state.hash_one(self)
            }
        })+
    };
}
hash_value!(i8, i16, i32, i64, i128, i256, u8, u16, u32, u64);
hash_value!(bool, str, [u8], IntervalDayTime, IntervalMonthDayNano);

macro_rules! hash_float_value {
    ($(($t:ty, $i:ty)),+) => {
        $(impl HashValue for $t {
            fn hash_one(&self, state: &RandomState) -> u64 {
                state.hash_one(<$i>::from_ne_bytes(self.to_ne_bytes()))
            }
        })+
    };
}
hash_float_value!((half::f16, u16), (f32, u32), (f64, u64));

/// Builds hash values of PrimitiveArray and writes them into `hashes_buffer`
/// If `rehash==true` this combines the previous hash value in the buffer
/// with the new hash using `combine_hashes`
#[cfg(not(feature = "force_hash_collisions"))]
fn hash_array_primitive<T>(
    array: &PrimitiveArray<T>,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
    rehash: bool,
) where
    T: ArrowPrimitiveType<Native: HashValue>,
{
    assert_eq!(
        hashes_buffer.len(),
        array.len(),
        "hashes_buffer and array should be of equal length"
    );

    if array.null_count() == 0 {
        if rehash {
            for (hash, &value) in hashes_buffer.iter_mut().zip(array.values().iter()) {
                *hash = combine_hashes(value.hash_one(random_state), *hash);
            }
        } else {
            for (hash, &value) in hashes_buffer.iter_mut().zip(array.values().iter()) {
                *hash = value.hash_one(random_state);
            }
        }
    } else if rehash {
        for (i, hash) in hashes_buffer.iter_mut().enumerate() {
            if !array.is_null(i) {
                let value = unsafe { array.value_unchecked(i) };
                *hash = combine_hashes(value.hash_one(random_state), *hash);
            }
        }
    } else {
        for (i, hash) in hashes_buffer.iter_mut().enumerate() {
            if !array.is_null(i) {
                let value = unsafe { array.value_unchecked(i) };
                *hash = value.hash_one(random_state);
            }
        }
    }
}

/// Hashes one array into the `hashes_buffer`
/// If `rehash==true` this combines the previous hash value in the buffer
/// with the new hash using `combine_hashes`
#[cfg(not(feature = "force_hash_collisions"))]
fn hash_array<T>(
    array: T,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
    rehash: bool,
) where
    T: ArrayAccessor,
    T::Item: HashValue,
{
    assert_eq!(
        hashes_buffer.len(),
        array.len(),
        "hashes_buffer and array should be of equal length"
    );

    if array.null_count() == 0 {
        if rehash {
            for (i, hash) in hashes_buffer.iter_mut().enumerate() {
                let value = unsafe { array.value_unchecked(i) };
                *hash = combine_hashes(value.hash_one(random_state), *hash);
            }
        } else {
            for (i, hash) in hashes_buffer.iter_mut().enumerate() {
                let value = unsafe { array.value_unchecked(i) };
                *hash = value.hash_one(random_state);
            }
        }
    } else if rehash {
        for (i, hash) in hashes_buffer.iter_mut().enumerate() {
            if !array.is_null(i) {
                let value = unsafe { array.value_unchecked(i) };
                *hash = combine_hashes(value.hash_one(random_state), *hash);
            }
        }
    } else {
        for (i, hash) in hashes_buffer.iter_mut().enumerate() {
            if !array.is_null(i) {
                let value = unsafe { array.value_unchecked(i) };
                *hash = value.hash_one(random_state);
            }
        }
    }
}

/// Helper function to update hash for a dictionary key if the value is valid
#[cfg(not(feature = "force_hash_collisions"))]
#[inline]
fn update_hash_for_dict_key(
    hash: &mut u64,
    dict_hashes: &[u64],
    dict_values: &dyn Array,
    idx: usize,
    multi_col: bool,
) {
    if dict_values.is_valid(idx) {
        if multi_col {
            *hash = combine_hashes(dict_hashes[idx], *hash);
        } else {
            *hash = dict_hashes[idx];
        }
    }
    // no update for invalid dictionary value
}

/// Hash the values in a dictionary array
#[cfg(not(feature = "force_hash_collisions"))]
fn hash_dictionary<K: ArrowDictionaryKeyType>(
    array: &DictionaryArray<K>,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
    multi_col: bool,
) -> Result<()> {
    // Hash each dictionary value once, and then use that computed
    // hash for each key value to avoid a potentially expensive
    // redundant hashing for large dictionary elements (e.g. strings)
    let dict_values = Arc::clone(array.values());
    let mut dict_hashes = vec![0; dict_values.len()];
    create_hashes(&[dict_values], random_state, &mut dict_hashes)?;

    // combine hash for each index in values
    let dict_values = array.values();
    for (hash, key) in hashes_buffer.iter_mut().zip(array.keys().iter()) {
        if let Some(key) = key {
            let idx = key.as_usize();
            update_hash_for_dict_key(
                hash,
                &dict_hashes,
                dict_values.as_ref(),
                idx,
                multi_col,
            );
        } // no update for Null key
    }
    Ok(())
}

#[cfg(not(feature = "force_hash_collisions"))]
fn hash_struct_array(
    array: &StructArray,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
) -> Result<()> {
    let nulls = array.nulls();
    let row_len = array.len();

    let valid_row_indices: Vec<usize> = if let Some(nulls) = nulls {
        nulls.valid_indices().collect()
    } else {
        (0..row_len).collect()
    };

    // Create hashes for each row that combines the hashes over all the column at that row.
    let mut values_hashes = vec![0u64; row_len];
    create_hashes(array.columns(), random_state, &mut values_hashes)?;

    for i in valid_row_indices {
        let hash = &mut hashes_buffer[i];
        *hash = combine_hashes(*hash, values_hashes[i]);
    }

    Ok(())
}

// only adding this `cfg` b/c this function is only used with this `cfg`
#[cfg(not(feature = "force_hash_collisions"))]
fn hash_map_array(
    array: &MapArray,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
) -> Result<()> {
    let nulls = array.nulls();
    let offsets = array.offsets();

    // Create hashes for each entry in each row
    let mut values_hashes = vec![0u64; array.entries().len()];
    create_hashes(array.entries().columns(), random_state, &mut values_hashes)?;

    // Combine the hashes for entries on each row with each other and previous hash for that row
    if let Some(nulls) = nulls {
        for (i, (start, stop)) in offsets.iter().zip(offsets.iter().skip(1)).enumerate() {
            if nulls.is_valid(i) {
                let hash = &mut hashes_buffer[i];
                for values_hash in &values_hashes[start.as_usize()..stop.as_usize()] {
                    *hash = combine_hashes(*hash, *values_hash);
                }
            }
        }
    } else {
        for (i, (start, stop)) in offsets.iter().zip(offsets.iter().skip(1)).enumerate() {
            let hash = &mut hashes_buffer[i];
            for values_hash in &values_hashes[start.as_usize()..stop.as_usize()] {
                *hash = combine_hashes(*hash, *values_hash);
            }
        }
    }

    Ok(())
}

#[cfg(not(feature = "force_hash_collisions"))]
fn hash_list_array<OffsetSize>(
    array: &GenericListArray<OffsetSize>,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
) -> Result<()>
where
    OffsetSize: OffsetSizeTrait,
{
    let values = Arc::clone(array.values());
    let offsets = array.value_offsets();
    let nulls = array.nulls();
    let mut values_hashes = vec![0u64; values.len()];
    create_hashes(&[values], random_state, &mut values_hashes)?;
    if let Some(nulls) = nulls {
        for (i, (start, stop)) in offsets.iter().zip(offsets.iter().skip(1)).enumerate() {
            if nulls.is_valid(i) {
                let hash = &mut hashes_buffer[i];
                for values_hash in &values_hashes[start.as_usize()..stop.as_usize()] {
                    *hash = combine_hashes(*hash, *values_hash);
                }
            }
        }
    } else {
        for (i, (start, stop)) in offsets.iter().zip(offsets.iter().skip(1)).enumerate() {
            let hash = &mut hashes_buffer[i];
            for values_hash in &values_hashes[start.as_usize()..stop.as_usize()] {
                *hash = combine_hashes(*hash, *values_hash);
            }
        }
    }
    Ok(())
}

#[cfg(not(feature = "force_hash_collisions"))]
fn hash_fixed_list_array(
    array: &FixedSizeListArray,
    random_state: &RandomState,
    hashes_buffer: &mut [u64],
) -> Result<()> {
    let values = Arc::clone(array.values());
    let value_length = array.value_length() as usize;
    let nulls = array.nulls();
    let mut values_hashes = vec![0u64; values.len()];
    create_hashes(&[values], random_state, &mut values_hashes)?;
    if let Some(nulls) = nulls {
        for i in 0..array.len() {
            if nulls.is_valid(i) {
                let hash = &mut hashes_buffer[i];
                for values_hash in
                    &values_hashes[i * value_length..(i + 1) * value_length]
                {
                    *hash = combine_hashes(*hash, *values_hash);
                }
            }
        }
    } else {
        for i in 0..array.len() {
            let hash = &mut hashes_buffer[i];
            for values_hash in &values_hashes[i * value_length..(i + 1) * value_length] {
                *hash = combine_hashes(*hash, *values_hash);
            }
        }
    }
    Ok(())
}

/// Test version of `create_hashes` that produces the same value for
/// all hashes (to test collisions)
///
/// See comments on `hashes_buffer` for more details
#[cfg(feature = "force_hash_collisions")]
pub fn create_hashes<'a>(
    _arrays: &[ArrayRef],
    _random_state: &RandomState,
    hashes_buffer: &'a mut Vec<u64>,
) -> Result<&'a mut Vec<u64>> {
    for hash in hashes_buffer.iter_mut() {
        *hash = 0
    }
    Ok(hashes_buffer)
}

/// Creates hash values for every row, based on the values in the
/// columns.
///
/// The number of rows to hash is determined by `hashes_buffer.len()`.
/// `hashes_buffer` should be pre-sized appropriately
#[cfg(not(feature = "force_hash_collisions"))]
pub fn create_hashes<'a>(
    arrays: &[ArrayRef],
    random_state: &RandomState,
    hashes_buffer: &'a mut Vec<u64>,
) -> Result<&'a mut Vec<u64>> {
    for (i, col) in arrays.iter().enumerate() {
        let array = col.as_ref();
        // combine hashes with `combine_hashes` for all columns besides the first
        let rehash = i >= 1;
        downcast_primitive_array! {
            array => hash_array_primitive(array, random_state, hashes_buffer, rehash),
            DataType::Null => hash_null(random_state, hashes_buffer, rehash),
            DataType::Boolean => hash_array(as_boolean_array(array)?, random_state, hashes_buffer, rehash),
            DataType::Utf8 => hash_array(as_string_array(array)?, random_state, hashes_buffer, rehash),
            DataType::Utf8View => hash_array(as_string_view_array(array)?, random_state, hashes_buffer, rehash),
            DataType::LargeUtf8 => hash_array(as_largestring_array(array), random_state, hashes_buffer, rehash),
            DataType::Binary => hash_array(as_generic_binary_array::<i32>(array)?, random_state, hashes_buffer, rehash),
            DataType::BinaryView => hash_array(as_binary_view_array(array)?, random_state, hashes_buffer, rehash),
            DataType::LargeBinary => hash_array(as_generic_binary_array::<i64>(array)?, random_state, hashes_buffer, rehash),
            DataType::FixedSizeBinary(_) => {
                let array: &FixedSizeBinaryArray = array.as_any().downcast_ref().unwrap();
                hash_array(array, random_state, hashes_buffer, rehash)
            }
            DataType::Dictionary(_, _) => downcast_dictionary_array! {
                array => hash_dictionary(array, random_state, hashes_buffer, rehash)?,
                _ => unreachable!()
            }
            DataType::Struct(_) => {
                let array = as_struct_array(array)?;
                hash_struct_array(array, random_state, hashes_buffer)?;
            }
            DataType::List(_) => {
                let array = as_list_array(array)?;
                hash_list_array(array, random_state, hashes_buffer)?;
            }
            DataType::LargeList(_) => {
                let array = as_large_list_array(array)?;
                hash_list_array(array, random_state, hashes_buffer)?;
            }
            DataType::Map(_, _) => {
                let array = as_map_array(array)?;
                hash_map_array(array, random_state, hashes_buffer)?;
            }
            DataType::FixedSizeList(_,_) => {
                let array = as_fixed_size_list_array(array)?;
                hash_fixed_list_array(array, random_state, hashes_buffer)?;
            }
            _ => {
                // This is internal because we should have caught this before.
                return _internal_err!(
                    "Unsupported data type in hasher: {}",
                    col.data_type()
                );
            }
        }
    }
    Ok(hashes_buffer)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::*;
    #[cfg(not(feature = "force_hash_collisions"))]
    use arrow::datatypes::*;

    use super::*;

    #[test]
    fn create_hashes_for_decimal_array() -> Result<()> {
        let array = vec![1, 2, 3, 4]
            .into_iter()
            .map(Some)
            .collect::<Decimal128Array>()
            .with_precision_and_scale(20, 3)
            .unwrap();
        let array_ref = Arc::new(array);
        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let hashes_buff = &mut vec![0; array_ref.len()];
        let hashes = create_hashes(&[array_ref], &random_state, hashes_buff)?;
        assert_eq!(hashes.len(), 4);
        Ok(())
    }

    #[test]
    fn create_hashes_for_empty_fixed_size_lit() -> Result<()> {
        let empty_array = FixedSizeListBuilder::new(StringBuilder::new(), 1).finish();
        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let hashes_buff = &mut vec![0; 0];
        let hashes = create_hashes(&[Arc::new(empty_array)], &random_state, hashes_buff)?;
        assert_eq!(hashes, &Vec::<u64>::new());
        Ok(())
    }

    #[test]
    fn create_hashes_for_float_arrays() -> Result<()> {
        let f32_arr = Arc::new(Float32Array::from(vec![0.12, 0.5, 1f32, 444.7]));
        let f64_arr = Arc::new(Float64Array::from(vec![0.12, 0.5, 1f64, 444.7]));

        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let hashes_buff = &mut vec![0; f32_arr.len()];
        let hashes = create_hashes(&[f32_arr], &random_state, hashes_buff)?;
        assert_eq!(hashes.len(), 4,);

        let hashes = create_hashes(&[f64_arr], &random_state, hashes_buff)?;
        assert_eq!(hashes.len(), 4,);

        Ok(())
    }

    macro_rules! create_hash_binary {
        ($NAME:ident, $ARRAY:ty) => {
            #[cfg(not(feature = "force_hash_collisions"))]
            #[test]
            fn $NAME() {
                let binary = [
                    Some(b"short".to_byte_slice()),
                    None,
                    Some(b"long but different 12 bytes string"),
                    Some(b"short2"),
                    Some(b"Longer than 12 bytes string"),
                    Some(b"short"),
                    Some(b"Longer than 12 bytes string"),
                ];

                let binary_array = Arc::new(binary.iter().cloned().collect::<$ARRAY>());
                let ref_array = Arc::new(binary.iter().cloned().collect::<BinaryArray>());

                let random_state = RandomState::with_seeds(0, 0, 0, 0);

                let mut binary_hashes = vec![0; binary.len()];
                create_hashes(&[binary_array], &random_state, &mut binary_hashes)
                    .unwrap();

                let mut ref_hashes = vec![0; binary.len()];
                create_hashes(&[ref_array], &random_state, &mut ref_hashes).unwrap();

                // Null values result in a zero hash,
                for (val, hash) in binary.iter().zip(binary_hashes.iter()) {
                    match val {
                        Some(_) => assert_ne!(*hash, 0),
                        None => assert_eq!(*hash, 0),
                    }
                }

                // same logical values should hash to the same hash value
                assert_eq!(binary_hashes, ref_hashes);

                // Same values should map to same hash values
                assert_eq!(binary[0], binary[5]);
                assert_eq!(binary[4], binary[6]);

                // different binary should map to different hash values
                assert_ne!(binary[0], binary[2]);
            }
        };
    }

    create_hash_binary!(binary_array, BinaryArray);
    create_hash_binary!(binary_view_array, BinaryViewArray);

    #[test]
    fn create_hashes_fixed_size_binary() -> Result<()> {
        let input_arg = vec![vec![1, 2], vec![5, 6], vec![5, 6]];
        let fixed_size_binary_array =
            Arc::new(FixedSizeBinaryArray::try_from_iter(input_arg.into_iter()).unwrap());

        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let hashes_buff = &mut vec![0; fixed_size_binary_array.len()];
        let hashes =
            create_hashes(&[fixed_size_binary_array], &random_state, hashes_buff)?;
        assert_eq!(hashes.len(), 3,);

        Ok(())
    }

    macro_rules! create_hash_string {
        ($NAME:ident, $ARRAY:ty) => {
            #[cfg(not(feature = "force_hash_collisions"))]
            #[test]
            fn $NAME() {
                let strings = [
                    Some("short"),
                    None,
                    Some("long but different 12 bytes string"),
                    Some("short2"),
                    Some("Longer than 12 bytes string"),
                    Some("short"),
                    Some("Longer than 12 bytes string"),
                ];

                let string_array = Arc::new(strings.iter().cloned().collect::<$ARRAY>());
                let dict_array = Arc::new(
                    strings
                        .iter()
                        .cloned()
                        .collect::<DictionaryArray<Int8Type>>(),
                );

                let random_state = RandomState::with_seeds(0, 0, 0, 0);

                let mut string_hashes = vec![0; strings.len()];
                create_hashes(&[string_array], &random_state, &mut string_hashes)
                    .unwrap();

                let mut dict_hashes = vec![0; strings.len()];
                create_hashes(&[dict_array], &random_state, &mut dict_hashes).unwrap();

                // Null values result in a zero hash,
                for (val, hash) in strings.iter().zip(string_hashes.iter()) {
                    match val {
                        Some(_) => assert_ne!(*hash, 0),
                        None => assert_eq!(*hash, 0),
                    }
                }

                // same logical values should hash to the same hash value
                assert_eq!(string_hashes, dict_hashes);

                // Same values should map to same hash values
                assert_eq!(strings[0], strings[5]);
                assert_eq!(strings[4], strings[6]);

                // different strings should map to different hash values
                assert_ne!(strings[0], strings[2]);
            }
        };
    }

    create_hash_string!(string_array, StringArray);
    create_hash_string!(large_string_array, LargeStringArray);
    create_hash_string!(string_view_array, StringArray);
    create_hash_string!(dict_string_array, DictionaryArray<Int8Type>);

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_dict_arrays() {
        let strings = [Some("foo"), None, Some("bar"), Some("foo"), None];

        let string_array = Arc::new(strings.iter().cloned().collect::<StringArray>());
        let dict_array = Arc::new(
            strings
                .iter()
                .cloned()
                .collect::<DictionaryArray<Int8Type>>(),
        );

        let random_state = RandomState::with_seeds(0, 0, 0, 0);

        let mut string_hashes = vec![0; strings.len()];
        create_hashes(&[string_array], &random_state, &mut string_hashes).unwrap();

        let mut dict_hashes = vec![0; strings.len()];
        create_hashes(&[dict_array], &random_state, &mut dict_hashes).unwrap();

        // Null values result in a zero hash,
        for (val, hash) in strings.iter().zip(string_hashes.iter()) {
            match val {
                Some(_) => assert_ne!(*hash, 0),
                None => assert_eq!(*hash, 0),
            }
        }

        // same logical values should hash to the same hash value
        assert_eq!(string_hashes, dict_hashes);

        // Same values should map to same hash values
        assert_eq!(strings[1], strings[4]);
        assert_eq!(dict_hashes[1], dict_hashes[4]);
        assert_eq!(strings[0], strings[3]);
        assert_eq!(dict_hashes[0], dict_hashes[3]);

        // different strings should map to different hash values
        assert_ne!(strings[0], strings[2]);
        assert_ne!(dict_hashes[0], dict_hashes[2]);
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_list_arrays() {
        let data = vec![
            Some(vec![Some(0), Some(1), Some(2)]),
            None,
            Some(vec![Some(3), None, Some(5)]),
            Some(vec![Some(3), None, Some(5)]),
            None,
            Some(vec![Some(0), Some(1), Some(2)]),
            Some(vec![]),
        ];
        let list_array =
            Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>(data)) as ArrayRef;
        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let mut hashes = vec![0; list_array.len()];
        create_hashes(&[list_array], &random_state, &mut hashes).unwrap();
        assert_eq!(hashes[0], hashes[5]);
        assert_eq!(hashes[1], hashes[4]);
        assert_eq!(hashes[2], hashes[3]);
        assert_eq!(hashes[1], hashes[6]); // null vs empty list
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_fixed_size_list_arrays() {
        let data = vec![
            Some(vec![Some(0), Some(1), Some(2)]),
            None,
            Some(vec![Some(3), None, Some(5)]),
            Some(vec![Some(3), None, Some(5)]),
            None,
            Some(vec![Some(0), Some(1), Some(2)]),
        ];
        let list_array =
            Arc::new(FixedSizeListArray::from_iter_primitive::<Int32Type, _, _>(
                data, 3,
            )) as ArrayRef;
        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let mut hashes = vec![0; list_array.len()];
        create_hashes(&[list_array], &random_state, &mut hashes).unwrap();
        assert_eq!(hashes[0], hashes[5]);
        assert_eq!(hashes[1], hashes[4]);
        assert_eq!(hashes[2], hashes[3]);
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_struct_arrays() {
        use arrow::buffer::Buffer;

        let boolarr = Arc::new(BooleanArray::from(vec![
            false, false, true, true, true, true,
        ]));
        let i32arr = Arc::new(Int32Array::from(vec![10, 10, 20, 20, 30, 31]));

        let struct_array = StructArray::from((
            vec![
                (
                    Arc::new(Field::new("bool", DataType::Boolean, false)),
                    Arc::clone(&boolarr) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("i32", DataType::Int32, false)),
                    Arc::clone(&i32arr) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("i32", DataType::Int32, false)),
                    Arc::clone(&i32arr) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("bool", DataType::Boolean, false)),
                    Arc::clone(&boolarr) as ArrayRef,
                ),
            ],
            Buffer::from(&[0b001011]),
        ));

        assert!(struct_array.is_valid(0));
        assert!(struct_array.is_valid(1));
        assert!(struct_array.is_null(2));
        assert!(struct_array.is_valid(3));
        assert!(struct_array.is_null(4));
        assert!(struct_array.is_null(5));

        let array = Arc::new(struct_array) as ArrayRef;

        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let mut hashes = vec![0; array.len()];
        create_hashes(&[array], &random_state, &mut hashes).unwrap();
        assert_eq!(hashes[0], hashes[1]);
        // same value but the third row ( hashes[2] ) is null
        assert_ne!(hashes[2], hashes[3]);
        // different values but both are null
        assert_eq!(hashes[4], hashes[5]);
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_struct_arrays_more_column_than_row() {
        let struct_array = StructArray::from(vec![
            (
                Arc::new(Field::new("bool", DataType::Boolean, false)),
                Arc::new(BooleanArray::from(vec![false, false])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("i32-1", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![10, 10])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("i32-2", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![10, 10])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("i32-3", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![10, 10])) as ArrayRef,
            ),
        ]);

        assert!(struct_array.is_valid(0));
        assert!(struct_array.is_valid(1));

        let array = Arc::new(struct_array) as ArrayRef;
        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let mut hashes = vec![0; array.len()];
        create_hashes(&[array], &random_state, &mut hashes).unwrap();
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_hashes_for_map_arrays() {
        let mut builder =
            MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
        // Row 0
        builder.keys().append_value("key1");
        builder.keys().append_value("key2");
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true).unwrap();
        // Row 1
        builder.keys().append_value("key1");
        builder.keys().append_value("key2");
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true).unwrap();
        // Row 2
        builder.keys().append_value("key1");
        builder.keys().append_value("key2");
        builder.values().append_value(1);
        builder.values().append_value(3);
        builder.append(true).unwrap();
        // Row 3
        builder.keys().append_value("key1");
        builder.keys().append_value("key3");
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true).unwrap();
        // Row 4
        builder.keys().append_value("key1");
        builder.values().append_value(1);
        builder.append(true).unwrap();
        // Row 5
        builder.keys().append_value("key1");
        builder.values().append_null();
        builder.append(true).unwrap();
        // Row 6
        builder.append(true).unwrap();
        // Row 7
        builder.keys().append_value("key1");
        builder.values().append_value(1);
        builder.append(false).unwrap();

        let array = Arc::new(builder.finish()) as ArrayRef;

        let random_state = RandomState::with_seeds(0, 0, 0, 0);
        let mut hashes = vec![0; array.len()];
        create_hashes(&[array], &random_state, &mut hashes).unwrap();
        assert_eq!(hashes[0], hashes[1]); // same value
        assert_ne!(hashes[0], hashes[2]); // different value
        assert_ne!(hashes[0], hashes[3]); // different key
        assert_ne!(hashes[0], hashes[4]); // missing an entry
        assert_ne!(hashes[4], hashes[5]); // filled vs null value
        assert_eq!(hashes[6], hashes[7]); // empty vs null map
    }

    #[test]
    // Tests actual values of hashes, which are different if forcing collisions
    #[cfg(not(feature = "force_hash_collisions"))]
    fn create_multi_column_hash_for_dict_arrays() {
        let strings1 = [Some("foo"), None, Some("bar")];
        let strings2 = [Some("blarg"), Some("blah"), None];

        let string_array = Arc::new(strings1.iter().cloned().collect::<StringArray>());
        let dict_array = Arc::new(
            strings2
                .iter()
                .cloned()
                .collect::<DictionaryArray<Int32Type>>(),
        );

        let random_state = RandomState::with_seeds(0, 0, 0, 0);

        let mut one_col_hashes = vec![0; strings1.len()];
        create_hashes(
            &[Arc::clone(&dict_array) as ArrayRef],
            &random_state,
            &mut one_col_hashes,
        )
        .unwrap();

        let mut two_col_hashes = vec![0; strings1.len()];
        create_hashes(
            &[dict_array, string_array],
            &random_state,
            &mut two_col_hashes,
        )
        .unwrap();

        assert_eq!(one_col_hashes.len(), 3);
        assert_eq!(two_col_hashes.len(), 3);

        assert_ne!(one_col_hashes, two_col_hashes);
    }
}
