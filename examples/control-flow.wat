(module
  (func (export "max") (param $left i32) (param $right i32) (result i32)
    local.get $left
    local.get $right
    i32.gt_s
    if (result i32)
      local.get $left
    else
      local.get $right
    end
  )
)
