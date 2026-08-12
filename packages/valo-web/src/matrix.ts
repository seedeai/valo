export type Affine = readonly [
  a: number,
  b: number,
  c: number,
  d: number,
  translateX: number,
  translateY: number,
];

export const identity: Affine = [1, 0, 0, 1, 0, 0];

export function multiply(left: Affine, right: Affine): Affine {
  return [
    left[0] * right[0] + left[2] * right[1],
    left[1] * right[0] + left[3] * right[1],
    left[0] * right[2] + left[2] * right[3],
    left[1] * right[2] + left[3] * right[3],
    left[0] * right[4] + left[2] * right[5] + left[4],
    left[1] * right[4] + left[3] * right[5] + left[5],
  ];
}

export function inverse(matrix: Affine): Affine | undefined {
  const determinant = matrix[0] * matrix[3] - matrix[1] * matrix[2];
  if (!Number.isFinite(determinant) || determinant === 0) return undefined;
  const reciprocal = 1 / determinant;
  return [
    matrix[3] * reciprocal,
    -matrix[1] * reciprocal,
    -matrix[2] * reciprocal,
    matrix[0] * reciprocal,
    (matrix[2] * matrix[5] - matrix[3] * matrix[4]) * reciprocal,
    (matrix[1] * matrix[4] - matrix[0] * matrix[5]) * reciprocal,
  ];
}

export function mapPoint(matrix: Affine, x: number, y: number): [number, number] {
  return [
    matrix[0] * x + matrix[2] * y + matrix[4],
    matrix[1] * x + matrix[3] * y + matrix[5],
  ];
}

export function asDomMatrix(matrix: Affine): DOMMatrix {
  return new DOMMatrix([...matrix]);
}
