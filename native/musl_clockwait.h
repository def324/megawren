// Included into every C++ file of the static musl build.
//
// musl has no pthread_cond_clockwait. Without it, libstdc++ converts a
// steady_clock deadline to system_clock time, and time_point::max() overflows
// into the past. The SDK waits until max() to mean "until notified", so its
// threads spun instead of sleeping and could starve shutdown of their mutex.
// This makes libstdc++ pass steady_clock deadlines to the function below.
#pragma once
#include <pthread.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif
int megawren_pthread_cond_clockwait(pthread_cond_t *cond, pthread_mutex_t *mutex,
                                    clockid_t clock, const struct timespec *deadline);
#ifdef __cplusplus
}
#endif

#define pthread_cond_clockwait megawren_pthread_cond_clockwait
#define _GLIBCXX_USE_PTHREAD_COND_CLOCKWAIT 1
