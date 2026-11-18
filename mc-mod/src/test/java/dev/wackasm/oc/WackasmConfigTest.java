package dev.wackasm.oc;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.Properties;

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** {@link WackasmConfig#apply}: values are read, clamped, and fall back when missing. */
class WackasmConfigTest {
    @AfterEach
    void reset() {
        WackasmConfig.apply(new Properties());
    }

    private static Properties props(String... pairs) {
        Properties p = new Properties();
        for (int i = 0; i < pairs.length; i += 2) p.setProperty(pairs[i], pairs[i + 1]);
        return p;
    }

    @Test
    void missingKeysUseTheDefaults() {
        WackasmConfig.apply(new Properties());
        assertEquals(1.0, WackasmConfig.callBudgetMultiplier);
        assertEquals(1.8, WackasmConfig.memoryMultiplier);
        assertEquals(1, WackasmConfig.asyncCallsPerTick);
        assertEquals(5_000_000, WackasmConfig.fuelPerSlice);
    }

    @Test
    void valuesAreReadAndClamped() {
        WackasmConfig.apply(props("callBudgetMultiplier", " 2.5 ", "memoryMultiplier", "9",
                "asyncCallsPerTick", "16", "fuelPerSlice", "5"));
        assertEquals(2.5, WackasmConfig.callBudgetMultiplier);
        assertEquals(4.0, WackasmConfig.memoryMultiplier);
        assertEquals(16, WackasmConfig.asyncCallsPerTick);
        assertEquals(10_000, WackasmConfig.fuelPerSlice);
    }

    @Test
    void unreadableValuesFallBack() {
        WackasmConfig.apply(props("memoryMultiplier", "lots", "asyncCallsPerTick", "NaN"));
        assertEquals(1.8, WackasmConfig.memoryMultiplier);
        assertEquals(1, WackasmConfig.asyncCallsPerTick);
    }
}
