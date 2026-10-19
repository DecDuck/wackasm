(module
  (type $unary_i32 (func (param i32) (result i32)))
  (table $dispatch 2 funcref)

  (func $double (type $unary_i32)
    local.get 0
    i32.const 2
    i32.mul
  )

  (func $triple (type $unary_i32)
    local.get 0
    i32.const 3
    i32.mul
  )

  (elem (i32.const 0) $double $triple)

  (func (export "apply") (param $function_index i32) (param $value i32) (result i32)
    local.get $value
    local.get $function_index
    call_indirect (type $unary_i32)
  )
)
