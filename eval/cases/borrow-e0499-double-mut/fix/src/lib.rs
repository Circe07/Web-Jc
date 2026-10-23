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
        let i = a.1 * self.width + a.0;
        let j = b.1 * self.width + b.0;
        self.cells.swap(i, j);
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
