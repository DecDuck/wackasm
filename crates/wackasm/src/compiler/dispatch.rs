#[cfg(feature = "compact-instructions")]
use std::{fmt, mem::MaybeUninit};

use serde::{Deserialize, Serialize};

#[cfg(feature = "compact-instructions")]
use crate::dispatch::{HANDLERS, RuntimeOpcode};
use crate::{compiler::WackasmOperator, dispatch::WackasmOperatorHandler};

#[cfg(feature = "compact-instructions")]
#[repr(C)]
pub(super) struct RuntimeOperands([MaybeUninit<u8>; 12]);

#[cfg_attr(feature = "compact-instructions", repr(C, align(8)))]
#[cfg_attr(not(feature = "compact-instructions"), derive(Debug, Clone))]
#[derive(Serialize, Deserialize)]
#[serde(from = "WackasmOperator", into = "WackasmOperator")]
pub(crate) struct RuntimeWackasmOperator {
    #[cfg(not(feature = "compact-instructions"))]
    handler: WackasmOperatorHandler,
    #[cfg(not(feature = "compact-instructions"))]
    pub(crate) op: WackasmOperator,
    #[cfg(feature = "compact-instructions")]
    pub(super) operands: RuntimeOperands,
    #[cfg(feature = "compact-instructions")]
    pub(super) opcode: RuntimeOpcode,
}

impl RuntimeWackasmOperator {
    #[inline(always)]
    pub(crate) fn handler(&self) -> WackasmOperatorHandler {
        #[cfg(feature = "compact-instructions")]
        {
            HANDLERS[self.opcode as usize]
        }
        #[cfg(not(feature = "compact-instructions"))]
        {
            self.handler
        }
    }

    #[cfg(not(feature = "compact-instructions"))]
    pub(crate) fn op(&self) -> WackasmOperator {
        self.op.clone()
    }

    #[cfg(feature = "compact-instructions")]
    /// # Safety
    /// The caller must select the operand type belonging to this instruction's opcode.
    #[inline(always)]
    pub(crate) unsafe fn operands<T>(&self) -> &T {
        // The first field of this 8-byte-aligned instruction is its operand storage.
        unsafe { &*self.operands.0.as_ptr().cast::<T>() }
    }

    #[cfg(feature = "compact-instructions")]
    /// # Safety
    /// The opcode must identify the supplied operand type.
    pub(super) unsafe fn new<T>(opcode: RuntimeOpcode, operands: T) -> Self {
        assert!(std::mem::size_of::<T>() <= 12);
        let mut storage = RuntimeOperands([MaybeUninit::uninit(); 12]);
        unsafe { storage.0.as_mut_ptr().cast::<T>().write_unaligned(operands) };
        Self {
            opcode,
            operands: storage,
        }
    }
}

#[cfg(feature = "compact-instructions")]
impl Clone for RuntimeWackasmOperator {
    fn clone(&self) -> Self {
        self.op().into()
    }
}

#[cfg(feature = "compact-instructions")]
impl fmt::Debug for RuntimeWackasmOperator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.op().fmt(formatter)
    }
}

#[cfg(not(feature = "compact-instructions"))]
impl From<WackasmOperator> for RuntimeWackasmOperator {
    fn from(op: WackasmOperator) -> Self {
        Self {
            handler: crate::dispatch::handler_for(&op),
            op,
        }
    }
}

#[cfg(not(feature = "compact-instructions"))]
impl From<RuntimeWackasmOperator> for WackasmOperator {
    fn from(op: RuntimeWackasmOperator) -> Self {
        op.op
    }
}

macro_rules! operators {
    ($($variant:ident $({$($field:ident: $ty:ty),* $(,)?})?),* $(,)?) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub enum WackasmOperator {
            $($variant $({$($field: $ty),*})?),*
        }

        #[cfg(feature = "compact-instructions")]
        pub(crate) mod operands {
            use super::*;

            $(
                #[derive(Clone)]
                pub(crate) struct $variant {
                    $($(pub(crate) $field: $ty,)*)?
                }
                const _: () = assert!(std::mem::size_of::<$variant>() <= 12);
                const _: () = assert!(std::mem::align_of::<$variant>() <= 8);
            )*
        }

        paste::paste! {
            #[cfg(feature = "compact-instructions")]
            impl From<WackasmOperator> for super::RuntimeWackasmOperator {
                fn from(op: WackasmOperator) -> Self {
                    let opcode = crate::dispatch::opcode_for(&op);
                    match op {
                        $(WackasmOperator::$variant { $($($field,)*)? } =>
                            unsafe { Self::new(opcode, operands::$variant { $($($field,)*)? }) },)*
                    }
                }
            }

            #[cfg(feature = "compact-instructions")]
            impl super::RuntimeWackasmOperator {
                pub(crate) fn op(&self) -> WackasmOperator {
                    unsafe {
                        match self.opcode {
                            $(crate::dispatch::RuntimeOpcode::$variant
                            | crate::dispatch::RuntimeOpcode::[<$variant Memory0>] => {
                                let value = self.operands::<operands::$variant>();
                                let operands::$variant { $($($field,)*)? } = (*value).clone();
                                WackasmOperator::$variant { $($($field,)*)? }
                            },)*
                        }
                    }
                }
            }

            #[cfg(feature = "compact-instructions")]
            impl From<super::RuntimeWackasmOperator> for WackasmOperator {
                fn from(op: super::RuntimeWackasmOperator) -> Self {
                    let op = std::mem::ManuallyDrop::new(op);
                    unsafe {
                        match op.opcode {
                            $(crate::dispatch::RuntimeOpcode::$variant
                            | crate::dispatch::RuntimeOpcode::[<$variant Memory0>] => {
                                let operands::$variant { $($($field,)*)? } =
                                    std::ptr::read(op.operands::<operands::$variant>());
                                WackasmOperator::$variant { $($($field,)*)? }
                            },)*
                        }
                    }
                }
            }

            #[cfg(feature = "compact-instructions")]
            impl Drop for super::RuntimeWackasmOperator {
                fn drop(&mut self) {
                    match self.opcode {
                        $(crate::dispatch::RuntimeOpcode::$variant
                        | crate::dispatch::RuntimeOpcode::[<$variant Memory0>] => {
                            if std::mem::needs_drop::<operands::$variant>() {
                                unsafe {
                                    drop(std::ptr::read(self.operands::<operands::$variant>()));
                                }
                            }
                        },)*
                    }
                }
            }
        }

        #[cfg(test)]
        #[test]
        fn every_runtime_opcode_preserves_portable_operands_and_ownership() {
            use super::dispatch::tests::{check, sample};

            for seed in 0..9 {
                $(check(WackasmOperator::$variant {
                    $($($field: sample::<$ty>(seed),)*)?
                });)*
            }
        }
    };
}

pub(super) use operators;

const _: () = assert!(std::mem::size_of::<RuntimeWackasmOperator>() <= 24);
#[cfg(feature = "compact-instructions")]
const _: () = assert!(std::mem::size_of::<RuntimeWackasmOperator>() == 16);
#[cfg(feature = "compact-instructions")]
const _: () = assert!(std::mem::offset_of!(RuntimeWackasmOperator, operands) == 0);

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::compiler::{I32Comparison, I64Comparison, WackasmMemArg};

    pub(crate) trait Sample {
        fn sample(seed: u32) -> Self;
    }

    pub(crate) fn sample<T: Sample>(seed: u32) -> T {
        T::sample(seed)
    }

    impl Sample for u16 {
        fn sample(seed: u32) -> Self {
            u16::MAX.wrapping_sub(seed as u16)
        }
    }

    impl Sample for u32 {
        fn sample(seed: u32) -> Self {
            u32::MAX.wrapping_sub(seed.wrapping_mul(0x1020_3040))
        }
    }

    impl Sample for i32 {
        fn sample(seed: u32) -> Self {
            match seed % 3 {
                0 => i32::MIN,
                1 => i32::MAX,
                _ => -1,
            }
        }
    }

    impl Sample for bool {
        fn sample(seed: u32) -> Self {
            seed & 1 != 0
        }
    }

    impl Sample for [u32; 2] {
        fn sample(seed: u32) -> Self {
            [u32::sample(seed), u32::sample(seed.wrapping_add(1))]
        }
    }

    impl Sample for WackasmMemArg {
        fn sample(seed: u32) -> Self {
            Self {
                memory: seed & 1,
                offset: u32::sample(seed),
            }
        }
    }

    impl Sample for I32Comparison {
        fn sample(seed: u32) -> Self {
            use I32Comparison::*;
            [Eq, Ne, LtS, LtU, GtS, GtU, GeS, GeU, LeU][seed as usize % 9]
        }
    }

    impl Sample for I64Comparison {
        fn sample(seed: u32) -> Self {
            use I64Comparison::*;
            [Eq, Ne, GtS, LtU, GtU, LeU, GeU][seed as usize % 7]
        }
    }

    impl Sample for Box<String> {
        fn sample(seed: u32) -> Self {
            Box::new(format!("unsupported operation {seed}\0with owned text"))
        }
    }

    pub(crate) fn check(op: WackasmOperator) {
        let expected = bitcode::serialize(&op).unwrap();
        let runtime = RuntimeWackasmOperator::from(op);
        assert_eq!(bitcode::serialize(&runtime).unwrap(), expected);
        let cloned = runtime.clone();
        drop(runtime);
        assert_eq!(bitcode::serialize(&cloned.op()).unwrap(), expected);
        let restored: RuntimeWackasmOperator = bitcode::deserialize(&expected).unwrap();
        assert_eq!(bitcode::serialize(&restored).unwrap(), expected);
        assert!(std::ptr::fn_addr_eq(cloned.handler(), restored.handler()));
        drop(restored);
        let moved: WackasmOperator = cloned.into();
        assert_eq!(bitcode::serialize(&moved).unwrap(), expected);
    }

    #[test]
    fn runtime_instruction_layout_matches_the_selected_dispatch_mode() {
        let expected = if cfg!(feature = "compact-instructions") {
            16
        } else {
            std::mem::size_of::<WackasmOperatorHandler>() + std::mem::size_of::<WackasmOperator>()
        };
        assert_eq!(std::mem::size_of::<RuntimeWackasmOperator>(), expected);
        assert_eq!(
            std::mem::size_of::<[RuntimeWackasmOperator; 4]>(),
            4 * expected
        );
        #[cfg(feature = "compact-instructions")]
        assert_eq!(std::mem::align_of::<RuntimeWackasmOperator>(), 8);
    }

    #[test]
    fn unsupported_instruction_owns_its_description_across_clone_and_restore() {
        let op = RuntimeWackasmOperator::from(WackasmOperator::Unsupported {
            description: Box::new("not implemented".to_owned()),
        });
        check(op.op());
        let bytes = bitcode::serialize(&op).unwrap();
        drop(op);
        let restored: RuntimeWackasmOperator = bitcode::deserialize(&bytes).unwrap();
        let mut module =
            crate::WackasmModule::new(&wat::parse_str("(module (func (export \"run\")))").unwrap())
                .unwrap();
        let crate::functions::WackasmFunction::Internal(body) =
            &mut module.functions.function_bodies[0]
        else {
            panic!("expected internal function")
        };
        body.runtime[0] = restored;
        let mut executor = crate::WackasmExecutor::new(module).unwrap();
        executor.call_export("run", vec![]).unwrap();
        let Err(crate::HaltError::Unsupported(error)) = executor.execute() else {
            panic!("expected unsupported instruction error")
        };
        assert!(error.to_string().contains("not implemented"));
    }
}
