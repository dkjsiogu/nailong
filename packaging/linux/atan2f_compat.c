/*
 * glibc 2.43 versions atan2f with GLIBC_2.43.  The launcher only uses it
 * through UI math, so route it to the long-standing double-precision entry
 * point.  This keeps the .deb usable on Ubuntu 24.04 (glibc 2.39).
 *
 * Linked only by scripts/build-deb.sh via -Wl,--wrap=atan2f.
 */
#include <math.h>

float __wrap_atan2f(float y, float x) {
    return (float)atan2((double)y, (double)x);
}
