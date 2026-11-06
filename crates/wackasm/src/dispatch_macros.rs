#[macro_export]
macro_rules! ops {
    (|$ctx:ident, $pc:ident, $regs:ident, $mem0:ident| const $memory0:ident, $spec:ident match $op:ident { $($arms:tt)* }) => {
        ops!(@parse [($) $ctx, $pc, $regs, $mem0, $memory0, $spec] [] $($arms)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? $([$kind:ident])? => $body:block $(,)? $($rest:tt)*
    ) => {
        ops!(@kind [$($kind)?] $params [$($acc)*] [$name $variant [$($($field)*)?]] $body $($rest)*);
    };
    // Each specialization is `[index MEMORY0 SPEC]`; `@select` maps an operator to its index.
    (@kind [] $params:tt [$($acc:tt)*] $head:tt $body:tt $($rest:tt)*) => {
        ops!(@parse $params [$($acc)* [$head [] [[0 false 0]] $body]] $($rest)*);
    };
    (@kind [memory] $params:tt [$($acc:tt)*] $head:tt $body:tt $($rest:tt)*) => {
        ops!(@parse $params [$($acc)* [$head [memory] [[0 false 0] [1 true 0]] $body]] $($rest)*);
    };
    (@kind [branch] $params:tt [$($acc:tt)*] $head:tt $body:tt $($rest:tt)*) => {
        ops!(@parse $params [$($acc)* [$head [branch] [[0 false 0] [1 false 1]] $body]] $($rest)*);
    };
    (@kind [compare] $params:tt [$($acc:tt)*] $head:tt $body:tt $($rest:tt)*) => {
        ops!(@parse $params [$($acc)* [$head [compare] [
            [0 false 0] [1 false 1] [2 false 2] [3 false 3] [4 false 4]
            [5 false 5] [6 false 6] [7 false 7] [8 false 8] [9 false 9]
            [10 false 10] [11 false 11] [12 false 12] [13 false 13] [14 false 14]
            [15 false 15] [16 false 16] [17 false 17] [18 false 18] [19 false 19]
        ] $body]] $($rest)*);
    };
    (@select $op:ident, $variant:ident, []) => { 0u8 };
    (@select $op:ident, $variant:ident, [memory]) => {{
        let WackasmOperator::$variant { memarg, .. } = $op else { unreachable!() };
        (memarg.memory == 0) as u8
    }};
    (@select $op:ident, $variant:ident, [branch]) => {{
        let WackasmOperator::$variant { offset, .. } = $op else { unreachable!() };
        branch_spec(*offset)
    }};
    (@select $op:ident, $variant:ident, [compare]) => {{
        let WackasmOperator::$variant { kind, invert, offset, .. } = $op else { unreachable!() };
        kind.code(*invert) << 1 | branch_spec(*offset)
    }};
    (@parse [($d:tt) $ctx:ident, $pc:ident, $regs:ident, $mem0:ident, $memory0:ident, $spec:ident] [$(
        [[$name:ident $variant:ident [$($field:ident)*]] [$($kind:ident)?] [$([$index:literal $memory:literal $specialization:literal])*] $body:tt]
    )*]) => {
        paste::paste! {
            #[cfg(feature = "compact-instructions")]
            #[repr(u16)]
            #[derive(Clone, Copy)]
            // One ID per specialization keeps handler-table indexing direct.
            #[allow(dead_code)]
            pub(crate) enum RuntimeOpcode {
                $($([<$variant Spec $index>],)*)*
            }

            #[cfg(feature = "compact-instructions")]
            impl RuntimeOpcode {
                /// The first specialization of this instruction's portable operator.
                pub(crate) fn portable(self) -> Self {
                    match self {
                        $($(Self::[<$variant Spec $index>])|* => Self::[<$variant Spec0>],)*
                    }
                }
            }

            #[cfg(feature = "compact-instructions")]
            pub(crate) fn opcode_for(op: &WackasmOperator) -> RuntimeOpcode {
                match op {
                    $(WackasmOperator::$variant { .. } => match ops!(@select op, $variant, [$($kind)?]) {
                        $($index => RuntimeOpcode::[<$variant Spec $index>],)*
                        _ => unreachable!(),
                    },)*
                }
            }
        }

        #[cfg(feature = "compact-instructions")]
        pub(crate) static HANDLERS: [WackasmOperatorHandler; [$($(stringify!($index),)*)*].len()] =
            [$($($name::<$memory, $specialization>,)*)*];

        #[cfg(not(feature = "compact-instructions"))]
        pub(crate) fn handler_for(op: &WackasmOperator) -> WackasmOperatorHandler {
            match op {
                $(WackasmOperator::$variant { .. } => match ops!(@select op, $variant, [$($kind)?]) {
                    $($index => $name::<$memory, $specialization>,)*
                    _ => unreachable!(),
                },)*
            }
        }

        #[macro_export]
        macro_rules! dispatch {
            ($d c:expr, $d p:expr, $d r:expr, $d m:expr) => {{
                let pc: *const RuntimeWackasmOperator = $d p;
                let handler = unsafe { (*pc).handler() };
                become handler($d c, pc, $d r, $d m)
            }};
        }

        $(
            handler! {
                #[allow(unused_variables, unused_mut, unreachable_code)]
                fn $name<'a, const $memory0: bool, const $spec: u8>(
                    $ctx: &mut DispatchCtx<'a>,
                    $pc: *const RuntimeWackasmOperator,
                    $regs: *mut Value,
                    mut $mem0: MemoryView,
                ) {
                    #[cfg(feature = "compact-instructions")]
                    let operands = unsafe {
                        (*$pc).operands::<crate::compiler::operands::$variant>()
                    };
                    #[cfg(feature = "compact-instructions")]
                    let crate::compiler::operands::$variant { $($field,)* .. } = operands;
                    #[cfg(not(feature = "compact-instructions"))]
                    let WackasmOperator::$variant { $($field,)* .. } = (unsafe { &(*$pc).op }) else {
                        unsafe { std::hint::unreachable_unchecked() }
                    };
                    $body;
                    dispatch!($ctx, unsafe { $pc.add(1) }, $regs, $mem0)
                }
            }
        )*
    };
}
