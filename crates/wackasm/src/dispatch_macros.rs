#[macro_export]
macro_rules! ops {
    (|$ctx:ident, $pc:ident, $regs:ident| match $op:ident { $($arms:tt)* }) => {
        ops!(@parse [($) $ctx, $pc, $regs] [] $($arms)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? => $body:block $(,)? $($rest:tt)*
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] $body]] $($rest)*);
    };
    (@parse [($d:tt) $ctx:ident, $pc:ident, $regs:ident] [$(
        [$name:ident $variant:ident [$($field:ident)*] $body:tt]
    )*]) => {
        pub(crate) fn handler_for(op: &WackasmOperator) -> WackasmOperatorHandler {
            match op {
                $( WackasmOperator::$variant { .. } => $name, )*
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
            fn $name<'a>(
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
