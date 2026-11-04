(module
  (func $make_value (result i32)
    (local $value i32)

    i32.const 40
    local.set $value
    local.get $value
    i32.const 2
    i32.add
  )

  (func (export "sum_call_results") (result i32)
    (local $first i32)
    (local $second i32)

    call $make_value
    local.set $first
    call $make_value
    local.set $second

    local.get $first
    local.get $second
    i32.add
  )
)
