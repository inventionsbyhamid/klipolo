use klipolo_engine::render::lut::parse_cube;

const IDENTITY_2: &str = "TITLE \"id\"\n# comment\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

#[test]
fn parses_a_cube_file_red_fastest() {
    let lut = parse_cube(IDENTITY_2).unwrap();
    assert_eq!(lut.size, 2);
    assert_eq!(lut.data[1], [1.0, 0.0, 0.0]);
    assert_eq!(lut.data[6], [0.0, 1.0, 1.0]);
}

#[test]
fn rejects_incomplete_or_1d_luts() {
    assert_eq!(
        parse_cube("LUT_3D_SIZE 2\n0 0 0\n").unwrap_err(),
        "This LUT file is incomplete."
    );
    assert_eq!(
        parse_cube("LUT_1D_SIZE 16\n").unwrap_err(),
        "Only 3D LUTs (.cube) are supported."
    );
    assert_eq!(
        parse_cube("0 0 0\n").unwrap_err(),
        "This LUT file has no LUT_3D_SIZE line."
    );
}
