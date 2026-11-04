(module
  (memory (export "memory") 1)

  (func (export "store_then_load")
    (param $address i32)
    (param $value i32)
    (result i32)
    local.get $address
    local.get $value
    i32.store

    local.get $address
    i32.load
  )
)
