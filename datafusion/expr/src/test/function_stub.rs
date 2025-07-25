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

//! Aggregate function stubs for test in expr / optimizer.
//!
//! These are used to avoid a dependence on `datafusion-functions-aggregate` which live in a different crate

use std::any::Any;

use arrow::datatypes::{
    DataType, FieldRef, DECIMAL128_MAX_PRECISION, DECIMAL256_MAX_PRECISION,
};

use datafusion_common::{exec_err, not_impl_err, utils::take_function_args, Result};

use crate::type_coercion::aggregates::{avg_return_type, coerce_avg_type, NUMERICS};
use crate::Volatility::Immutable;
use crate::{
    expr::AggregateFunction,
    function::{AccumulatorArgs, StateFieldsArgs},
    utils::AggregateOrderSensitivity,
    Accumulator, AggregateUDFImpl, Expr, GroupsAccumulator, ReversedUDAF, Signature,
};

macro_rules! create_func {
    ($UDAF:ty, $AGGREGATE_UDF_FN:ident) => {
        paste::paste! {
            #[doc = concat!("AggregateFunction that returns a [AggregateUDF](crate::AggregateUDF) for [`", stringify!($UDAF), "`]")]
            pub fn $AGGREGATE_UDF_FN() -> std::sync::Arc<crate::AggregateUDF> {
                // Singleton instance of [$UDAF], ensures the UDAF is only created once
                static INSTANCE: std::sync::LazyLock<std::sync::Arc<crate::AggregateUDF>> =
                    std::sync::LazyLock::new(|| {
                        std::sync::Arc::new(crate::AggregateUDF::from(<$UDAF>::default()))
                    });
                std::sync::Arc::clone(&INSTANCE)
            }
        }
    }
}

create_func!(Sum, sum_udaf);

pub fn sum(expr: Expr) -> Expr {
    Expr::AggregateFunction(AggregateFunction::new_udf(
        sum_udaf(),
        vec![expr],
        false,
        None,
        vec![],
        None,
    ))
}

create_func!(Count, count_udaf);

pub fn count(expr: Expr) -> Expr {
    Expr::AggregateFunction(AggregateFunction::new_udf(
        count_udaf(),
        vec![expr],
        false,
        None,
        vec![],
        None,
    ))
}

create_func!(Avg, avg_udaf);

pub fn avg(expr: Expr) -> Expr {
    Expr::AggregateFunction(AggregateFunction::new_udf(
        avg_udaf(),
        vec![expr],
        false,
        None,
        vec![],
        None,
    ))
}

/// Stub `sum` used for optimizer testing
#[derive(Debug)]
pub struct Sum {
    signature: Signature,
}

impl Sum {
    pub fn new() -> Self {
        Self {
            signature: Signature::user_defined(Immutable),
        }
    }
}

impl Default for Sum {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateUDFImpl for Sum {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "sum"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        let [array] = take_function_args(self.name(), arg_types)?;

        // Refer to https://www.postgresql.org/docs/8.2/functions-aggregate.html doc
        // smallint, int, bigint, real, double precision, decimal, or interval.

        fn coerced_type(data_type: &DataType) -> Result<DataType> {
            match data_type {
                DataType::Dictionary(_, v) => coerced_type(v),
                // in the spark, the result type is DECIMAL(min(38,precision+10), s)
                // ref: https://github.com/apache/spark/blob/fcf636d9eb8d645c24be3db2d599aba2d7e2955a/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/expressions/aggregate/Sum.scala#L66
                DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => {
                    Ok(data_type.clone())
                }
                dt if dt.is_signed_integer() => Ok(DataType::Int64),
                dt if dt.is_unsigned_integer() => Ok(DataType::UInt64),
                dt if dt.is_floating() => Ok(DataType::Float64),
                _ => exec_err!("Sum not supported for {}", data_type),
            }
        }

        Ok(vec![coerced_type(array)?])
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        match &arg_types[0] {
            DataType::Int64 => Ok(DataType::Int64),
            DataType::UInt64 => Ok(DataType::UInt64),
            DataType::Float64 => Ok(DataType::Float64),
            DataType::Decimal128(precision, scale) => {
                // in the spark, the result type is DECIMAL(min(38,precision+10), s)
                // ref: https://github.com/apache/spark/blob/fcf636d9eb8d645c24be3db2d599aba2d7e2955a/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/expressions/aggregate/Sum.scala#L66
                let new_precision = DECIMAL128_MAX_PRECISION.min(*precision + 10);
                Ok(DataType::Decimal128(new_precision, *scale))
            }
            DataType::Decimal256(precision, scale) => {
                // in the spark, the result type is DECIMAL(min(38,precision+10), s)
                // ref: https://github.com/apache/spark/blob/fcf636d9eb8d645c24be3db2d599aba2d7e2955a/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/expressions/aggregate/Sum.scala#L66
                let new_precision = DECIMAL256_MAX_PRECISION.min(*precision + 10);
                Ok(DataType::Decimal256(new_precision, *scale))
            }
            other => {
                exec_err!("[return_type] SUM not supported for {}", other)
            }
        }
    }

    fn accumulator(&self, _args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        unreachable!("stub should not have accumulate()")
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        unreachable!("stub should not have state_fields()")
    }

    fn groups_accumulator_supported(&self, _args: AccumulatorArgs) -> bool {
        false
    }

    fn create_groups_accumulator(
        &self,
        _args: AccumulatorArgs,
    ) -> Result<Box<dyn GroupsAccumulator>> {
        unreachable!("stub should not have accumulate()")
    }

    fn reverse_expr(&self) -> ReversedUDAF {
        ReversedUDAF::Identical
    }

    fn order_sensitivity(&self) -> AggregateOrderSensitivity {
        AggregateOrderSensitivity::Insensitive
    }
}

/// Testing stub implementation of COUNT aggregate
pub struct Count {
    signature: Signature,
    aliases: Vec<String>,
}

impl std::fmt::Debug for Count {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Count")
            .field("name", &self.name())
            .field("signature", &self.signature)
            .finish()
    }
}

impl Default for Count {
    fn default() -> Self {
        Self::new()
    }
}

impl Count {
    pub fn new() -> Self {
        Self {
            aliases: vec!["count".to_string()],
            signature: Signature::variadic_any(Immutable),
        }
    }
}

impl AggregateUDFImpl for Count {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "COUNT"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int64)
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        not_impl_err!("no impl for stub")
    }

    fn accumulator(&self, _acc_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn aliases(&self) -> &[String] {
        &self.aliases
    }

    fn create_groups_accumulator(
        &self,
        _args: AccumulatorArgs,
    ) -> Result<Box<dyn GroupsAccumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn reverse_expr(&self) -> ReversedUDAF {
        ReversedUDAF::Identical
    }
}

create_func!(Min, min_udaf);

pub fn min(expr: Expr) -> Expr {
    Expr::AggregateFunction(AggregateFunction::new_udf(
        min_udaf(),
        vec![expr],
        false,
        None,
        vec![],
        None,
    ))
}

/// Testing stub implementation of Min aggregate
pub struct Min {
    signature: Signature,
}

impl std::fmt::Debug for Min {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Min")
            .field("name", &self.name())
            .field("signature", &self.signature)
            .finish()
    }
}

impl Default for Min {
    fn default() -> Self {
        Self::new()
    }
}

impl Min {
    pub fn new() -> Self {
        Self {
            signature: Signature::variadic_any(Immutable),
        }
    }
}

impl AggregateUDFImpl for Min {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "min"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int64)
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        not_impl_err!("no impl for stub")
    }

    fn accumulator(&self, _acc_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn create_groups_accumulator(
        &self,
        _args: AccumulatorArgs,
    ) -> Result<Box<dyn GroupsAccumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn reverse_expr(&self) -> ReversedUDAF {
        ReversedUDAF::Identical
    }
    fn is_descending(&self) -> Option<bool> {
        Some(false)
    }
}

create_func!(Max, max_udaf);

pub fn max(expr: Expr) -> Expr {
    Expr::AggregateFunction(AggregateFunction::new_udf(
        max_udaf(),
        vec![expr],
        false,
        None,
        vec![],
        None,
    ))
}

/// Testing stub implementation of MAX aggregate
pub struct Max {
    signature: Signature,
}

impl std::fmt::Debug for Max {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Max")
            .field("name", &self.name())
            .field("signature", &self.signature)
            .finish()
    }
}

impl Default for Max {
    fn default() -> Self {
        Self::new()
    }
}

impl Max {
    pub fn new() -> Self {
        Self {
            signature: Signature::variadic_any(Immutable),
        }
    }
}

impl AggregateUDFImpl for Max {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "max"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int64)
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        not_impl_err!("no impl for stub")
    }

    fn accumulator(&self, _acc_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn create_groups_accumulator(
        &self,
        _args: AccumulatorArgs,
    ) -> Result<Box<dyn GroupsAccumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn reverse_expr(&self) -> ReversedUDAF {
        ReversedUDAF::Identical
    }
    fn is_descending(&self) -> Option<bool> {
        Some(true)
    }
}

/// Testing stub implementation of avg aggregate
#[derive(Debug)]
pub struct Avg {
    signature: Signature,
    aliases: Vec<String>,
}

impl Avg {
    pub fn new() -> Self {
        Self {
            aliases: vec![String::from("mean")],
            signature: Signature::uniform(1, NUMERICS.to_vec(), Immutable),
        }
    }
}

impl Default for Avg {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateUDFImpl for Avg {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "avg"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        avg_return_type(self.name(), &arg_types[0])
    }

    fn accumulator(&self, _acc_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        not_impl_err!("no impl for stub")
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        not_impl_err!("no impl for stub")
    }

    fn aliases(&self) -> &[String] {
        &self.aliases
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        coerce_avg_type(self.name(), arg_types)
    }
}
