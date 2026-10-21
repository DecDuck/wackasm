use std::sync::Arc;

use anyhow::anyhow;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use self_cell::self_cell;
use wasmparser::{BinaryReader, FunctionBody, Operator, OperatorsReader};

use crate::HaltError;

type ParsedOperators<'a> = Vec<Operator<'a>>;

self_cell!(
    struct OperatorViewCell {
        owner: Vec<u8>,

        #[covariant]
        dependent: ParsedOperators,
    }

    impl {Debug}
);

/// Owns operator bytes and caches operators that borrow from those bytes.
#[derive(Clone, Debug)]
pub(crate) struct OperatorView {
    cell: Arc<OperatorViewCell>,
}

impl OperatorView {
    pub fn empty() -> Self {
        Self::from_raw(Vec::new()).expect("empty operator sequence is valid")
    }

    pub fn new(code: FunctionBody<'_>) -> Result<Self, HaltError> {
        let reader = code.get_binary_reader_for_operators()?;
        let operator_start = usize::try_from(reader.original_position() - code.range().start)?;
        let operators_raw = code
            .as_bytes()
            .get(operator_start..)
            .ok_or_else(|| anyhow!("operator start is outside the function body"))?
            .to_vec();
        Self::from_raw(operators_raw)
    }

    fn from_raw(operators_raw: Vec<u8>) -> Result<Self, HaltError> {
        let cell = OperatorViewCell::try_new(operators_raw, |bytes| {
            let reader = BinaryReader::new(bytes, 0);
            OperatorsReader::new(reader).into_iter().collect()
        })
        .map_err(|error| anyhow!("failed to parse cached operators: {error:?}"))?;

        Ok(Self {
            cell: Arc::new(cell),
        })
    }

    pub fn read(&self) -> impl Iterator<Item = Operator<'_>> + '_ {
        self.cell.borrow_dependent().iter().cloned()
    }

    pub fn get(&self, index: usize) -> Option<&Operator<'_>> {
        self.cell.borrow_dependent().get(index)
    }
}

impl Serialize for OperatorView {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.cell.borrow_owner().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OperatorView {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let operators_raw = Vec::<u8>::deserialize(deserializer)?;
        Self::from_raw(operators_raw).map_err(D::Error::custom)
    }
}