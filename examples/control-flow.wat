0    local.get $left
1    local.get $right
2    i32.gt_s
3    if (result i32)
4      local.get $left
5    else
6      local.get $right
7    end

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
