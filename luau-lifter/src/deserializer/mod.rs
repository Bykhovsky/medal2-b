use nom::{bytes::complete::take, IResult};
use nom_leb128::leb128_usize;

pub mod bytecode;
pub mod chunk;
pub mod constant;
pub mod function;
mod list;

fn parse_string(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
    let (input, length) = leb128_usize(input)?;
    let (input, bytes) = take(length)(input)?;
    Ok((input, bytes.to_owned()))
}

pub fn deserialize(bytecode: &[u8], encode_key: u8) -> Result<bytecode::Bytecode, String> {
    let version = bytecode
        .first()
        .copied()
        .ok_or_else(|| "bytecode is empty".to_string())?;

    if version != 0
        && !(bytecode::MIN_SUPPORTED_VERSION..=bytecode::MAX_SUPPORTED_VERSION).contains(&version)
    {
        return Err(format!(
            "unsupported bytecode version: {version} (supported: {}..={})",
            bytecode::MIN_SUPPORTED_VERSION,
            bytecode::MAX_SUPPORTED_VERSION
        ));
    }

    match bytecode::Bytecode::parse(bytecode, encode_key) {
        // Roblox bytecode providers can append host-specific data after the
        // serialized Luau chunk. The Luau chunk itself ends after the main
        // function id, so preserve the original deserializer's prefix-parsing
        // behavior instead of rejecting an otherwise valid chunk.
        Ok((_remaining, deserialized_bytecode)) => Ok(deserialized_bytecode),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{instruction::Instruction, op_code::OpCode};

    fn encoded_opcode(op_code: u8, encode_key: u8) -> u8 {
        (0..=u8::MAX)
            .find(|candidate| candidate.wrapping_mul(encode_key) == op_code)
            .expect("encode key must be invertible modulo 256")
    }

    fn abc_instruction(op_code: u8, encode_key: u8, a: u8, b: u8, c: u8) -> u32 {
        u32::from(encoded_opcode(op_code, encode_key))
            | (u32::from(a) << 8)
            | (u32::from(b) << 16)
            | (u32::from(c) << 24)
    }

    fn ad_instruction(op_code: u8, encode_key: u8, a: u8, d: i16) -> u32 {
        u32::from(encoded_opcode(op_code, encode_key))
            | (u32::from(a) << 8)
            | (u32::from(d as u16) << 16)
    }

    fn version_nine_chunk(encode_key: u8) -> Vec<u8> {
        let mut result = vec![
            9, // bytecode version
            3, // type encoding version
            1, 5, b'f', b'i', b'e', b'l', b'd', // string table
            0,    // end of userdata type remapping
            1,    // function count
            2, 1, 0, 0, 0, // function header
            0, // type info size
            3, // instruction word count
        ];

        result.extend_from_slice(
            &abc_instruction(OpCode::LOP_GETUDATAKS as u8, encode_key, 1, 0, 0).to_le_bytes(),
        );
        result.extend_from_slice(&(0xabcd_u32 << 16).to_le_bytes());
        result.extend_from_slice(
            &abc_instruction(OpCode::LOP_RETURN as u8, encode_key, 1, 2, 0).to_le_bytes(),
        );
        result.extend_from_slice(&[
            1, // constant count
            3, 1, // string constant, string table index 1
            0, // child function count
            0, // line defined
            0, // debug function name
            0, // no line info
            1, // debug info is present
            1, // local count
            1, // local name: string table index 1
            0, // local start pc
            3, // local end pc
            0, // local register
            0, // debug upvalue count
            0, // main function index
        ]);
        result
    }

    fn version_nine_prefilled_table_chunk(encode_key: u8) -> Vec<u8> {
        let mut result = vec![
            9, // bytecode version
            3, // type encoding version
            1, 6, b'a', b'n', b's', b'w', b'e', b'r', // string table
            0,    // end of userdata type remapping
            1,    // function count
            1, 0, 0, 0, 0, // function header
            0, // type info size
            2, // instruction word count
        ];

        result.extend_from_slice(
            &ad_instruction(OpCode::LOP_DUPTABLE as u8, encode_key, 0, 2).to_le_bytes(),
        );
        result.extend_from_slice(
            &abc_instruction(OpCode::LOP_RETURN as u8, encode_key, 0, 2, 0).to_le_bytes(),
        );
        result.extend_from_slice(&[
            3, // constant count
            3, 1, // constant 0: string table index 1
            9, 0, 42, // constant 1: positive integer 42
            8, 1, 0, // constant 2: table template, one entry, key constant 0
        ]);
        result.extend_from_slice(&1i32.to_le_bytes()); // value constant 1
        result.extend_from_slice(&[
            0, // child function count
            0, // line defined
            0, // debug function name
            0, // no line info
            0, // no debug info
            0, // main function index
        ]);
        result
    }

    #[test]
    fn deserializes_complete_version_nine_chunk() {
        let encode_key = 203;
        let bytecode = version_nine_chunk(encode_key);
        let parsed = deserialize(&bytecode, encode_key).unwrap();
        let bytecode::Bytecode::Chunk(chunk) = parsed else {
            panic!("expected a bytecode chunk")
        };

        assert_eq!(chunk.string_table, vec![b"field".to_vec()]);
        assert_eq!(chunk.functions.len(), 1);
        assert_eq!(chunk.functions[0].instructions.len(), 3);
        assert!(matches!(
            chunk.functions[0].instructions[0],
            Instruction::BC {
                op_code: OpCode::LOP_GETUDATAKS,
                a: 1,
                b: 0,
                aux: 0xabcd0000,
                ..
            }
        ));

        let source = crate::decompile_bytecode(&bytecode, encode_key);
        assert!(!source.contains("failed to deserialize"), "{source}");
        assert!(source.contains(".field"), "{source}");
    }

    #[test]
    fn decompiles_cumulative_version_seven_and_eight_features_in_version_nine() {
        let encode_key = 203;
        let source =
            crate::decompile_bytecode(&version_nine_prefilled_table_chunk(encode_key), encode_key);

        assert!(source.contains("answer"), "{source}");
        assert!(source.contains("42i"), "{source}");
    }

    #[test]
    fn accepts_host_data_after_a_complete_version_nine_chunk() {
        let encode_key = 203;
        let mut bytecode = version_nine_chunk(encode_key);
        bytecode.extend_from_slice(&[0xa5; 24]);

        let source = crate::decompile_bytecode(&bytecode, encode_key);

        assert!(!source.contains("failed to deserialize"), "{source}");
        assert!(source.contains(".field"), "{source}");
    }

    #[test]
    fn reports_future_versions_without_panicking_the_server_worker() {
        let result = std::panic::catch_unwind(|| crate::decompile_bytecode(&[10], 203));

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "failed to deserialize bytecode: unsupported bytecode version: 10 (supported: 4..=9)"
        );
    }
}

/*#[test]
fn main() -> anyhow::Result<()> {
    let compiler = Compiler::new()
        .set_debug_level(1).set_optimization_level(2);
    let bytecode = compiler.compile("asd = test");
    println!("{:#?}", bytecode);
    let deserialized = deserialize(&bytecode);
    println!("{:#?}", deserialized);
    Ok(())
}*/
