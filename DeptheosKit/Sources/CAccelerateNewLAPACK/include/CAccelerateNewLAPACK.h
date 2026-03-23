#ifndef CAccelerateNewLAPACK_h
#define CAccelerateNewLAPACK_h

// Opt into Apple's new (post-macOS 13.3) LAPACK ABI with 64-bit integers.
// This MUST be defined before including <Accelerate/Accelerate.h> so that
// the headers expose the non-deprecated `__LAPACK_int`-based prototypes.
#ifndef ACCELERATE_NEW_LAPACK
#define ACCELERATE_NEW_LAPACK
#endif
#ifndef ACCELERATE_LAPACK_ILP64
#define ACCELERATE_LAPACK_ILP64
#endif

#include <Accelerate/Accelerate.h>

#endif /* CAccelerateNewLAPACK_h */
