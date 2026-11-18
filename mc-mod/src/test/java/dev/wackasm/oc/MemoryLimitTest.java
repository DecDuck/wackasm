package dev.wackasm.oc;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

/** {@link WackasmArchitecture#memoryLimit}: installed RAM times the multiplier. */
class MemoryLimitTest {
    private static final long KIB = 1024;

    @Test
    void scalesInstalledRam() {
        // One tier 1 stick at the default 1.8: 345.6 KiB, so five 64 KiB pages fit.
        assertEquals(353_894, WackasmArchitecture.memoryLimit(192 * KIB, 1.8));
        assertEquals(192 * KIB, WackasmArchitecture.memoryLimit(192 * KIB, 1.0));
        assertEquals(2048 * KIB, WackasmArchitecture.memoryLimit(1024 * KIB, 2.0));
    }

    @Test
    void clampsTheMultiplier() {
        assertEquals(192 * KIB, WackasmArchitecture.memoryLimit(192 * KIB, 0.5));
        assertEquals(768 * KIB, WackasmArchitecture.memoryLimit(192 * KIB, 10.0));
        assertEquals(192 * KIB, WackasmArchitecture.memoryLimit(192 * KIB, Double.NaN));
    }

    @Test
    void noRamStaysZero() {
        assertEquals(0, WackasmArchitecture.memoryLimit(0, 1.8));
    }
}
