(module
  (func (export "sum_to_n") (param $n i32) (result i32)
    (local $sum i32)

    i32.const 0
    local.set $sum

    (block $exit
      (loop $again
        local.get $n
        i32.eqz
        br_if $exit

        local.get $sum
        local.get $n
        i32.add
        local.set $sum

        local.get $n
        i32.const 1
        i32.sub
        local.set $n
        br $again
      )
    )

    local.get $sum
  )
)
