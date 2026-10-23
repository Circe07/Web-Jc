/// A dense grid of integers stored row by row.
pub struct Grid {
    cells: Vec<i32>,
    width: usize,
}

impl Grid {
    /// Creates a zeroed grid.
    pub fn new(width: usize, height: usize) -> Self {
        Grid {
            cells: vec![0; width * height],
            width,
        }
    }

    /// Mutable access to one cell.
    pub fn cell_mut(&mut self, x: usize, y: usize) -> &mut i32 {
        &mut self.cells[y * self.width + x]
    }

    /// Exchanges the values of two cells.
    pub fn swap_cells(&mut self, a: (usize, usize), b: (usize, usize)) {
        let first = self.cell_mut(a.0, a.1);
        let second = self.cell_mut(b.0, b.1);
        std::mem::swap(first, second);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_two_cells() {
        let mut g = Grid::new(3, 2);
        *g.cell_mut(0, 0) = 7;
        *g.cell_mut(2, 1) = 9;
        g.swap_cells((0, 0), (2, 1));
        assert_eq!(*g.cell_mut(0, 0), 9);
        assert_eq!(*g.cell_mut(2, 1), 7);
    }
}
