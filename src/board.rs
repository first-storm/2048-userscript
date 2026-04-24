pub(crate) type Board = u64;
pub(crate) type Row = u16;

pub(crate) const ROW_MASK: Board = 0xffff;

pub(crate) fn reverse_row(row: Row) -> Row {
    ((row >> 12) & 0x000f) | ((row >> 4) & 0x00f0) | ((row << 4) & 0x0f00) | ((row << 12) & 0xf000)
}

pub(crate) fn unpack_col(row: Row) -> Board {
    let row = row as Board;
    (row & 0x000f) | ((row & 0x00f0) << 12) | ((row & 0x0f00) << 24) | ((row & 0xf000) << 36)
}

pub(crate) fn transpose(x: Board) -> Board {
    let a1 = x & 0xF0F00F0FF0F00F0F;
    let a2 = x & 0x0000F0F00000F0F0;
    let a3 = x & 0x0F0F00000F0F0000;
    let a = a1 | (a2 << 12) | (a3 >> 12);
    let b1 = a & 0xFF00FF0000FF00FF;
    let b2 = a & 0x00FF00FF00000000;
    let b3 = a & 0x00000000FF00FF00;
    b1 | (b2 >> 24) | (b3 << 24)
}

pub(crate) fn count_empty(mut x: Board) -> i32 {
    x |= (x >> 2) & 0x3333333333333333;
    x |= x >> 1;
    x = !x & 0x1111111111111111;
    x += x >> 32;
    x += x >> 16;
    x += x >> 8;
    x += x >> 4;
    (x & 0xf) as i32
}

pub(crate) fn count_distinct_tiles(mut board: Board) -> i32 {
    let mut bitset = 0u16;
    while board != 0 {
        bitset |= 1 << (board & 0xf);
        board >>= 4;
    }
    bitset >>= 1;

    let mut count = 0;
    while bitset != 0 {
        bitset &= bitset - 1;
        count += 1;
    }
    count
}
