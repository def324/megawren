// pthread_cond_clockwait for musl. See musl_clockwait.h.
#include <errno.h>
#include <stdint.h>

#include "musl_clockwait.h"

int megawren_pthread_cond_clockwait(pthread_cond_t *cond, pthread_mutex_t *mutex,
                                    clockid_t clock, const struct timespec *deadline)
{
    if (clock == CLOCK_REALTIME)
        return pthread_cond_timedwait(cond, mutex, deadline);
    if (clock != CLOCK_MONOTONIC)
        return EINVAL;

    struct timespec monotonic, realtime;
    clock_gettime(CLOCK_MONOTONIC, &monotonic);
    clock_gettime(CLOCK_REALTIME, &realtime);

    // Time left on the monotonic clock. Callers check their own clock after
    // waking, so capping a far deadline only adds a harmless wakeup.
    int64_t seconds = (int64_t)deadline->tv_sec - (int64_t)monotonic.tv_sec;
    long nanoseconds = deadline->tv_nsec - monotonic.tv_nsec;
    if (nanoseconds < 0) {
        nanoseconds += 1000000000L;
        seconds -= 1;
    }
    if (seconds < 0)
        return ETIMEDOUT;
    const int64_t ten_years = INT64_C(10) * 365 * 24 * 60 * 60;
    if (seconds > ten_years)
        seconds = ten_years;

    struct timespec until = {
        .tv_sec = (time_t)(realtime.tv_sec + seconds),
        .tv_nsec = realtime.tv_nsec + nanoseconds,
    };
    if (until.tv_nsec >= 1000000000L) {
        until.tv_nsec -= 1000000000L;
        until.tv_sec += 1;
    }
    return pthread_cond_timedwait(cond, mutex, &until);
}
