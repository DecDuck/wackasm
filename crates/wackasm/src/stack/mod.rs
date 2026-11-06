use wasmparser::ValType;

mod basic;

pub(crate) type FrameOperandStack = basic::OperandStack;

pub fn val_type_to_size(val_type: &ValType) -> usize {
    if matches!(val_type, ValType::V128) {
        2
    } else {
        1
    }
}

pub fn val_types_to_size(val_types: &[ValType]) -> usize {
    val_types.iter().map(val_type_to_size).sum()
}
