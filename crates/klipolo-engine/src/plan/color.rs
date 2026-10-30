/// `#RRGGBB` or `#RGB` as bytes; anything else is black (validation refuses it anyway).
pub fn parse_hex(s: &str) -> [u8; 3] {
    let hex = s.trim().trim_start_matches('#');
    let d: Vec<u8> = hex
        .chars()
        .filter_map(|c| c.to_digit(16).map(|v| v as u8))
        .collect();
    match (hex.len(), d.len()) {
        (6, 6) => [d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5]],
        (3, 3) => [d[0] * 17, d[1] * 17, d[2] * 17],
        _ => [0, 0, 0],
    }
}
