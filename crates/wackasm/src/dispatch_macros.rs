#[macro_export]
macro_rules! ops {
    (|$ctx:ident, $pc:ident, $regs:ident| const $memory0:ident match $op:ident { $($arms:tt)* }) => {
        ops!(@parse [($) $ctx, $pc, $regs, $memory0] [] $($arms)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? $([$memory:ident])? => $body:block $(,)? $($rest:tt)*
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] [$($memory)?] $body]] $($rest)*);
    };
    (@handler $op:ident, $variant:ident, $name:ident, []) => { $name::<false> };
    (@handler $op:ident, $variant:ident, $name:ident, [memory]) => {{
        let WackasmOperator::$variant { memarg, .. } = $op else {
            unreachable!()
        };
        if memarg.memory == 0 { $name::<true> } else { $name::<false> }
    }};
    (@parse [($d:tt) $ctx:ident, $pc:ident, $regs:ident, $memory0:ident] [$(
        [$name:ident $variant:ident [$($field:ident)*] [$($memory:ident)?] $body:tt]
    )*]) => {
        pub(crate) fn handler_for(op: &WackasmOperator) -> WackasmOperatorHandler {
            match op {
                $( WackasmOperator::$variant { .. } =>
                    ops!(@handler op, $variant, $name, [$($memory)?]), )*
            }
        }

        #[macro_export]
        macro_rules! dispatch {
            ($d c:expr, $d p:expr, $d r:expr) => {{
                let pc: *const RuntimeWackasmOperator = $d p;
                let handler = unsafe { (*pc).handler };
                become handler($d c, pc, $d r)
            }};
        }

        $(
            #[allow(unused_variables, unreachable_code)]
            fn $name<'a, const $memory0: bool>(
                $ctx: &mut DispatchCtx<'a>,
                $pc: *const RuntimeWackasmOperator,
                $regs: *mut Value,
            ) {
                let WackasmOperator::$variant { $($field,)* .. } = (unsafe { &(*$pc).op }) else {
                    unsafe { std::hint::unreachable_unchecked() }
                };
                $body;
                dispatch!($ctx, unsafe { $pc.add(1) }, $regs)
            }
        )*
    };
}
