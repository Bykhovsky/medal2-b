use super::list::parse_list;
use nom::{
    error::{Error, ErrorKind},
    number::complete::{le_f32, le_f64, le_i32, le_u32, le_u8},
    Err, IResult,
};
use nom_leb128::{leb128_u64, leb128_usize};

const CONSTANT_NIL: u8 = 0;
const CONSTANT_BOOLEAN: u8 = 1;
const CONSTANT_NUMBER: u8 = 2;
const CONSTANT_STRING: u8 = 3;
const CONSTANT_IMPORT: u8 = 4;
const CONSTANT_TABLE: u8 = 5;
const CONSTANT_CLOSURE: u8 = 6;
const CONSTANT_VECTOR: u8 = 7;
const CONSTANT_TABLE_WITH_CONSTANTS: u8 = 8;
const CONSTANT_INTEGER: u8 = 9;

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Nil,
    Boolean(bool),
    Number(f64),
    String(usize),
    Import(usize),
    Table(Vec<usize>),
    Closure(usize),
    Vector(f32, f32, f32, f32),
    TableWithConstants(Vec<(usize, Option<usize>)>),
    Integer(i64),
}

impl Constant {
    pub(crate) fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, tag) = le_u8(input)?;
        match tag {
            CONSTANT_NIL => Ok((input, Constant::Nil)),
            CONSTANT_BOOLEAN => {
                let (input, value) = le_u8(input)?;
                Ok((input, Constant::Boolean(value != 0u8)))
            }
            CONSTANT_NUMBER => {
                let (input, value) = le_f64(input)?;
                Ok((input, Constant::Number(value)))
            }
            CONSTANT_STRING => {
                let (input, string_index) = leb128_usize(input)?;
                Ok((input, Constant::String(string_index)))
            }
            CONSTANT_IMPORT => {
                let (input, import_index) = le_u32(input)?;
                Ok((input, Constant::Import(import_index as usize)))
            }
            CONSTANT_TABLE => {
                let (input, keys) = parse_list(input, leb128_usize)?;
                Ok((input, Constant::Table(keys)))
            }
            CONSTANT_CLOSURE => {
                let (input, f_id) = leb128_usize(input)?;
                Ok((input, Constant::Closure(f_id)))
            }
            CONSTANT_VECTOR => {
                let (input, x) = le_f32(input)?;
                let (input, y) = le_f32(input)?;
                let (input, z) = le_f32(input)?;
                let (input, w) = le_f32(input)?;
                Ok((input, Constant::Vector(x, y, z, w)))
            }
            CONSTANT_TABLE_WITH_CONSTANTS => {
                let (input, entries) = parse_list(input, |input| {
                    let (input, key_index) = leb128_usize(input)?;
                    let (input, value_index) = le_i32(input)?;
                    let value_index = usize::try_from(value_index).ok();
                    Ok((input, (key_index, value_index)))
                })?;
                Ok((input, Constant::TableWithConstants(entries)))
            }
            CONSTANT_INTEGER => {
                let (input, is_negative) = le_u8(input)?;
                if is_negative > 1 {
                    return Err(Err::Failure(Error::new(input, ErrorKind::Verify)));
                }

                let (input, magnitude) = leb128_u64(input)?;
                let value = match (is_negative, magnitude) {
                    (0, magnitude) if magnitude <= i64::MAX as u64 => magnitude as i64,
                    (1, magnitude) if magnitude <= (1u64 << 63) => magnitude.wrapping_neg() as i64,
                    _ => return Err(Err::Failure(Error::new(input, ErrorKind::Verify))),
                };
                Ok((input, Constant::Integer(value)))
            }
            _ => Err(Err::Failure(Error::new(input, ErrorKind::Alt))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut result = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            result.push(byte);
            if value == 0 {
                return result;
            }
        }
    }

    #[test]
    fn parses_prefilled_table_template() {
        let mut input = vec![CONSTANT_TABLE_WITH_CONSTANTS, 2, 1];
        input.extend_from_slice(&3i32.to_le_bytes());
        input.push(2);
        input.extend_from_slice(&(-1i32).to_le_bytes());

        let (remaining, constant) = Constant::parse(&input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(
            constant,
            Constant::TableWithConstants(vec![(1, Some(3)), (2, None)])
        );
    }

    #[test]
    fn parses_full_integer_range() {
        for (value, sign, magnitude) in [
            (i64::MAX, 0, i64::MAX as u64),
            (-42, 1, 42),
            (i64::MIN, 1, 1u64 << 63),
        ] {
            let mut input = vec![CONSTANT_INTEGER, sign];
            input.extend(encode_varint(magnitude));

            let (remaining, constant) = Constant::parse(&input).unwrap();
            assert!(remaining.is_empty());
            assert_eq!(constant, Constant::Integer(value));
        }
    }

    #[test]
    fn rejects_unknown_constant_tags_without_panicking() {
        let result = std::panic::catch_unwind(|| Constant::parse(&[u8::MAX]));

        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}
