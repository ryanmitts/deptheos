# Algorithms

How each stacking method and each aligner works, step by step, with the
formulas, the constants by their names in the code, and where each step
lives. The code is the authority: a page that disagrees with it is wrong
and should be fixed.

Each page ends with the published work it follows or is related to, cited
only for the part that work describes, and with links to the decision
records (`docs/decisions/`) that say why the method is built this way.

## Stacking methods

- [Pyramid](pyramid.md): Laplacian pyramid fusion with soft weights from a
  relative focus metric and a focus prior.
- [Depth map](depth-map.md): a per-pixel map of the sharpest frame,
  smoothed where the pick is unsure, then a blend by depth through a
  Laplacian pyramid.
- [Wavelet](wavelet.md): complex wavelet selection with color taken whole
  from one frame per pixel, adapted from focus-stack.

There is more than one method because they fail in different places. A
pyramid merge keeps fine crossing detail (hairs, bristles, antennae), but
its coarse levels can pick up a bright blurred edge and leave a halo. A
depth map gives each pixel one depth, so backgrounds and smooth surfaces
come out clean, but where a hair crosses a textured background it must
choose one surface. The wavelet
method picks one winning coefficient per subband and takes each pixel's
color whole from one frame.

## Aligners

- [ECC](ecc.md): a full affine fit by Enhanced Correlation Coefficient
  maximization, derived from OpenCV's `findTransformECC`.
- [Grid search](grid-search.md): a similarity transform found by scoring a
  shrinking grid of candidates, coarse to fine.

## Keeping a page current

Change the page in the same commit as the code it describes. A constant's
value, a formula or a step's order on a page should each be traceable to a
line of code. Add a paper to a page's related work only after reading the
part that is cited.
