(module
  (func $double (param $value i32) (result i32)
    local.get $value
    i32.const 2
    i32.mul
  )

  (func (export "quadruple") (param $value i32) (result i32)
    local.get $value
    call $double
    call $double
  )
)
