//! Sparse vector implementation for artist similarity
//!
//! Efficient representation for high-dimensional vectors where most values are zero.
//! Used to represent artist relationships where each dimension is another artist.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A sparse vector storing only non-zero values
///
/// Internally uses parallel vectors for indices and values for memory efficiency.
/// For operations, temporarily converts to HashMap for O(1) lookups.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SparseVector {
    /// Sorted indices of non-zero elements
    indices: Vec<u32>,
    /// Values corresponding to indices
    values: Vec<f32>,
}

impl SparseVector {
    /// Create an empty sparse vector
    pub fn new() -> Self {
        Self {
            indices: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Create a sparse vector with pre-allocated capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            indices: Vec::with_capacity(capacity),
            values: Vec::with_capacity(capacity),
        }
    }

    /// Create from parallel vectors (must be same length, indices should be sorted)
    pub fn from_parts(indices: Vec<u32>, values: Vec<f32>) -> Self {
        debug_assert_eq!(indices.len(), values.len());
        Self { indices, values }
    }

    /// Set a value at the given index
    ///
    /// If the value is 0 (or very close), removes the entry.
    /// If the index exists, updates the value.
    /// If the index doesn't exist, inserts in sorted order.
    pub fn set(&mut self, idx: u32, value: f32) {
        // Ignore near-zero values
        if value.abs() < 1e-9 {
            self.remove(idx);
            return;
        }

        match self.indices.binary_search(&idx) {
            Ok(pos) => {
                // Index exists, update value
                self.values[pos] = value;
            }
            Err(pos) => {
                // Index doesn't exist, insert at sorted position
                self.indices.insert(pos, idx);
                self.values.insert(pos, value);
            }
        }
    }

    /// Get the value at the given index (returns 0 if not present)
    pub fn get(&self, idx: u32) -> f32 {
        match self.indices.binary_search(&idx) {
            Ok(pos) => self.values[pos],
            Err(_) => 0.0,
        }
    }

    /// Remove an entry by index
    pub fn remove(&mut self, idx: u32) {
        if let Ok(pos) = self.indices.binary_search(&idx) {
            self.indices.remove(pos);
            self.values.remove(pos);
        }
    }

    /// Number of non-zero elements
    pub fn nnz(&self) -> usize {
        self.indices.len()
    }

    /// Check if vector is empty (all zeros)
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Get indices of non-zero elements
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Get values of non-zero elements
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Iterate over (index, value) pairs
    pub fn iter(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.indices
            .iter()
            .copied()
            .zip(self.values.iter().copied())
    }

    /// Add two sparse vectors element-wise
    pub fn add(&self, other: &SparseVector) -> SparseVector {
        let mut result = HashMap::new();

        // Add all elements from self
        for (idx, val) in self.iter() {
            *result.entry(idx).or_insert(0.0) += val;
        }

        // Add all elements from other
        for (idx, val) in other.iter() {
            *result.entry(idx).or_insert(0.0) += val;
        }

        // Convert back to sparse vector
        let mut indices: Vec<u32> = result.keys().copied().collect();
        indices.sort_unstable();

        let values: Vec<f32> = indices.iter().map(|idx| result[idx]).collect();

        SparseVector { indices, values }
    }

    /// Subtract another vector from this one (self - other)
    pub fn sub(&self, other: &SparseVector) -> SparseVector {
        let mut result = HashMap::new();

        for (idx, val) in self.iter() {
            *result.entry(idx).or_insert(0.0) += val;
        }

        for (idx, val) in other.iter() {
            *result.entry(idx).or_insert(0.0) -= val;
        }

        let mut indices: Vec<u32> = result
            .iter()
            .filter(|(_, v)| v.abs() > 1e-9)
            .map(|(k, _)| *k)
            .collect();
        indices.sort_unstable();

        let values: Vec<f32> = indices.iter().map(|idx| result[idx]).collect();

        SparseVector { indices, values }
    }

    /// Scale vector by a scalar
    pub fn scale(&self, scalar: f32) -> SparseVector {
        SparseVector {
            indices: self.indices.clone(),
            values: self.values.iter().map(|v| v * scalar).collect(),
        }
    }

    /// Compute dot product with another sparse vector
    pub fn dot(&self, other: &SparseVector) -> f32 {
        let mut sum = 0.0;
        let mut i = 0;
        let mut j = 0;

        // Merge-style iteration (both vectors are sorted by index)
        while i < self.indices.len() && j < other.indices.len() {
            match self.indices[i].cmp(&other.indices[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    sum += self.values[i] * other.values[j];
                    i += 1;
                    j += 1;
                }
            }
        }

        sum
    }

    /// Compute L2 norm (magnitude) of the vector
    pub fn magnitude(&self) -> f32 {
        self.values.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    /// Normalize the vector to unit length
    ///
    /// Returns a zero vector if magnitude is zero.
    pub fn normalize(&self) -> SparseVector {
        let mag = self.magnitude();
        if mag < 1e-9 {
            return SparseVector::new();
        }

        SparseVector {
            indices: self.indices.clone(),
            values: self.values.iter().map(|v| v / mag).collect(),
        }
    }

    /// Compute cosine similarity with another vector
    ///
    /// Returns 0 if either vector has zero magnitude.
    /// Result is in range [-1, 1], where 1 means identical direction.
    ///
    /// NOTE: ported for parity + test coverage, but NOT used for ranking
    /// (production ranks by summed relationship weight; epic decision D3).
    pub fn cosine_similarity(&self, other: &SparseVector) -> f32 {
        let dot = self.dot(other);
        let mag_self = self.magnitude();
        let mag_other = other.magnitude();

        if mag_self < 1e-9 || mag_other < 1e-9 {
            return 0.0;
        }

        dot / (mag_self * mag_other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut vec = SparseVector::new();
        vec.set(5, 1.0);
        vec.set(10, 2.0);
        vec.set(3, 0.5);

        assert_eq!(vec.get(5), 1.0);
        assert_eq!(vec.get(10), 2.0);
        assert_eq!(vec.get(3), 0.5);
        assert_eq!(vec.get(7), 0.0); // Not set
        assert_eq!(vec.nnz(), 3);

        // Indices should be sorted
        assert_eq!(vec.indices(), &[3, 5, 10]);
    }

    #[test]
    fn test_update_value() {
        let mut vec = SparseVector::new();
        vec.set(5, 1.0);
        vec.set(5, 2.0);

        assert_eq!(vec.get(5), 2.0);
        assert_eq!(vec.nnz(), 1);
    }

    #[test]
    fn test_remove_on_zero() {
        let mut vec = SparseVector::new();
        vec.set(5, 1.0);
        vec.set(5, 0.0);

        assert_eq!(vec.get(5), 0.0);
        assert_eq!(vec.nnz(), 0);
    }

    #[test]
    fn test_add() {
        let mut a = SparseVector::new();
        a.set(1, 1.0);
        a.set(3, 2.0);

        let mut b = SparseVector::new();
        b.set(2, 1.5);
        b.set(3, 0.5);

        let c = a.add(&b);

        assert_eq!(c.get(1), 1.0);
        assert_eq!(c.get(2), 1.5);
        assert_eq!(c.get(3), 2.5);
        assert_eq!(c.nnz(), 3);
    }

    #[test]
    fn test_dot_product() {
        let mut a = SparseVector::new();
        a.set(1, 2.0);
        a.set(3, 3.0);

        let mut b = SparseVector::new();
        b.set(1, 4.0);
        b.set(2, 5.0);
        b.set(3, 1.0);

        // dot = 2*4 + 3*1 = 11
        assert_eq!(a.dot(&b), 11.0);
    }

    #[test]
    fn test_magnitude() {
        let mut vec = SparseVector::new();
        vec.set(0, 3.0);
        vec.set(1, 4.0);

        // magnitude = sqrt(9 + 16) = 5
        assert!((vec.magnitude() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_normalize() {
        let mut vec = SparseVector::new();
        vec.set(0, 3.0);
        vec.set(1, 4.0);

        let normalized = vec.normalize();

        assert!((normalized.magnitude() - 1.0).abs() < 1e-6);
        assert!((normalized.get(0) - 0.6).abs() < 1e-6);
        assert!((normalized.get(1) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let mut vec = SparseVector::new();
        vec.set(0, 1.0);
        vec.set(1, 2.0);

        let sim = vec.cosine_similarity(&vec);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let mut a = SparseVector::new();
        a.set(0, 1.0);

        let mut b = SparseVector::new();
        b.set(1, 1.0);

        let sim = a.cosine_similarity(&b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_similar() {
        let mut a = SparseVector::new();
        a.set(0, 1.0);
        a.set(1, 1.0);

        let mut b = SparseVector::new();
        b.set(0, 1.0);
        b.set(1, 0.5);

        let sim = a.cosine_similarity(&b);
        // Should be high but not 1.0
        assert!(sim > 0.9);
        assert!(sim < 1.0);
    }

    #[test]
    fn test_scale() {
        let mut vec = SparseVector::new();
        vec.set(0, 2.0);
        vec.set(1, 3.0);

        let scaled = vec.scale(2.0);

        assert_eq!(scaled.get(0), 4.0);
        assert_eq!(scaled.get(1), 6.0);
    }

    #[test]
    fn test_from_parts() {
        let vec = SparseVector::from_parts(vec![1, 3, 5], vec![0.5, 1.0, 1.5]);

        assert_eq!(vec.get(1), 0.5);
        assert_eq!(vec.get(3), 1.0);
        assert_eq!(vec.get(5), 1.5);
        assert_eq!(vec.nnz(), 3);
    }

    #[test]
    fn test_empty_vector() {
        let vec = SparseVector::new();

        assert!(vec.is_empty());
        assert_eq!(vec.magnitude(), 0.0);
        assert_eq!(vec.normalize().nnz(), 0);
    }
}
