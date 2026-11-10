use tract_nnef::tensors::read_tensor;

fn header(dims: &[u32], bits: u32, kind: u16, bytes: u32) -> Vec<u8> {
    let mut h = vec![0; 128];
    h[..4].copy_from_slice(&[0x4e, 0xef, 1, 0]);
    h[4..8].copy_from_slice(&bytes.to_le_bytes());
    h[8..12].copy_from_slice(&(dims.len() as u32).to_le_bytes());
    for (i, dim) in dims.iter().enumerate() {
        h[12 + i * 4..16 + i * 4].copy_from_slice(&dim.to_le_bytes());
    }
    h[44..48].copy_from_slice(&bits.to_le_bytes());
    h[48..50].copy_from_slice(&kind.to_le_bytes());
    h
}

#[test]
fn overflowing_shape_is_rejected_before_allocation() {
    let result = read_tensor(header(&[u32::MAX; 8], 32, 0, 0).as_slice());
    assert!(result.unwrap_err().to_string().contains("overflows"));
}

#[test]
fn overflowing_byte_size_is_rejected_before_allocation() {
    assert!(read_tensor(header(&[1 << 31, 1 << 31], 64, 0, 0).as_slice()).is_err());
}

#[test]
fn missing_packed_bool_data_returns_error_without_underflow() {
    assert!(read_tensor(header(&[1], 1, 5, 0).as_slice()).is_err());
}

#[test]
fn valid_packed_bool_tensor_preserves_values() {
    let mut data = header(&[2], 1, 5, 1);
    data.push(0b1000_0000);
    assert_eq!(read_tensor(data.as_slice()).unwrap().as_slice::<bool>().unwrap(), &[true, false]);
}
