/* Feeds a deterministic 14 h, 1 Hz synthetic SGP40 raw-signal sequence
 * through Sensirion's reference VOC algorithm and prints "sraw,index"
 * per sample. Covers: slow baseline drift (rebases the mean estimator),
 * VOC events of several sizes and lengths, values outside (0, 65000),
 * values below/above the 20001..52767 clamp, more than 9.1 h of estimator
 * uptime (both uptimes reach the FIX16_MAX clamp at t = 32813), then a
 * 4.4 h elevated-VOC episode that keeps the estimator gated for over
 * 180 min (the gating-duration reset fires at t = 44569).
 *
 * With the argument "states" it instead runs the first 4 h, prints
 * GasIndexAlgorithm_get_states, restores them into a fresh instance with
 * GasIndexAlgorithm_set_states, and prints that instance's next 2 h. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "sensirion_gas_index_algorithm.h"

static uint32_t lcg_state = 12345u;

static int32_t noise(int32_t amplitude) {
    lcg_state = lcg_state * 1103515245u + 12345u;
    return (int32_t)((lcg_state >> 16) % (uint32_t)(2 * amplitude + 1)) - amplitude;
}

static int32_t synthetic_sraw(int32_t t) {
    int32_t s = 30000 + (t * 3000) / 10800 + noise(40);
    if (t >= 3000 && t < 3300) s -= 4000;
    if (t >= 6000 && t < 6100) s -= 12000;
    if (t >= 8000 && t < 8600) s -= (t - 8000) * 10;
    if (t >= 9500 && t < 9560) s += 9000;
    if (t >= 33000 && t < 49000) s -= 14000 + (t - 33000) / 4;
    if (t >= 5000 && t < 5120) s = 60000;
    if (t >= 7000 && t < 7120) s = 65535;
    if (t % 997 == 0) s = 0;
    if (t % 1499 == 0) s = 65535;
    if (t % 2311 == 0) s = -5;
    if (t % 1733 == 0) s = 15000;
    if (t % 2903 == 0) s = 60000;
    return s;
}

static int states(void) {
    GasIndexAlgorithmParams before, after;
    GasIndexAlgorithm_init(&before, GasIndexAlgorithm_ALGORITHM_TYPE_VOC);
    int32_t t = 1, index;
    for (; t <= 4 * 3600; t++) GasIndexAlgorithm_process(&before, synthetic_sraw(t), &index);
    float mean, std;
    GasIndexAlgorithm_get_states(&before, &mean, &std);
    /* %.9g round-trips a float exactly. */
    printf("state,%.9g,%.9g\n", mean, std);
    GasIndexAlgorithm_init(&after, GasIndexAlgorithm_ALGORITHM_TYPE_VOC);
    GasIndexAlgorithm_set_states(&after, mean, std);
    puts("sraw,index");
    for (; t <= 6 * 3600; t++) {
        int32_t sraw = synthetic_sraw(t);
        GasIndexAlgorithm_process(&after, sraw, &index);
        printf("%d,%d\n", sraw, index);
    }
    return 0;
}

int main(int argc, char** argv) {
    if (argc > 1 && strcmp(argv[1], "states") == 0) return states();
    GasIndexAlgorithmParams params;
    GasIndexAlgorithm_init(&params, GasIndexAlgorithm_ALGORITHM_TYPE_VOC);
    puts("sraw,index");
    for (int32_t t = 1; t <= 14 * 3600; t++) {
        int32_t sraw = synthetic_sraw(t);
        int32_t index;
        GasIndexAlgorithm_process(&params, sraw, &index);
        printf("%d,%d\n", sraw, index);
    }
    return 0;
}
