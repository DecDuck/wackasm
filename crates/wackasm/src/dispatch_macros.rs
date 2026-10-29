#[macro_export]
macro_rules! ops {
    (|$ctx:ident, $pc:ident, $stack:ident, $locals:ident| match $op:ident { $($arms:tt)* }) => {
        ops!(@parse [($) $ctx, $pc, $stack, $locals] [] $($arms)*);
    };

    // Munch arms one at a time: rustfmt omits the comma after block bodies but requires it
    // after expression bodies, and a single repetition can't express both.
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? => $body:block, $($rest:tt)*
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] $body]] $($rest)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? => $body:block $($rest:tt)*
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] $body]] $($rest)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? => $body:expr, $($rest:tt)*
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] $body]] $($rest)*);
    };
    (@parse $params:tt [$($acc:tt)*]
        $name:ident @ $variant:ident $({ $($field:ident),* $(,)? })? => $body:expr
    ) => {
        ops!(@parse $params [$($acc)* [$name $variant [$($($field)*)?] $body]]);
    };

    (@parse [($d:tt) $ctx:ident, $pc:ident, $stack:ident, $locals:ident] [$(
        [$name:ident $variant:ident [$($field:ident)*] $body:tt]
    )*]) => {
        pub(crate) fn handler_for(op: &WackasmOperator) -> WackasmOperatorHandler {
            match op {
                $( WackasmOperator::$variant { .. } => $name, )*
            }
        }

        #[macro_export]
        macro_rules! dispatch {
            ($d c:expr, $d p:expr, $d s:expr, $d l:expr) => {{
                let op: *const RuntimeWackasmOperator = unsafe { &*$d p };
                let handler = unsafe { (*op).handler };
                become handler($d c, $d p, $d s, $d l)
            }};
        }

        $(
            #[allow(unused_variables, unused_mut, unreachable_code)]
            fn $name<'a>(
                $ctx: &mut DispatchCtx<'a>,
                $pc: *const RuntimeWackasmOperator,
                mut $stack: StackHandle<'a>,
                $locals: LocalsHandle<'a>,
            ) -> Result<(), HaltError> {
                // `{ .. }` matches unit variants too.
                let WackasmOperator::$variant { $($field,)* .. } = (unsafe { &(*$pc).op }) else {
                    unsafe { unreachable_unchecked() }
                };
                $body;
                dispatch!($ctx, unsafe { $pc.add(1) }, $stack, $locals)
            }
        )*
    };
}
