package dev.wackasm.oc;

import static org.junit.jupiter.api.Assertions.*;

import java.lang.annotation.Annotation;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;

import li.cil.oc.api.machine.Callback;
import li.cil.oc.api.machine.Machine;
import li.cil.oc.api.network.Component;
import li.cil.oc.api.network.Network;
import li.cil.oc.api.network.Node;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** Checks what {@link WackasmArchitecture#call} charges, against fake OC objects. */
class CallBudgetTest {
    private final List<Double> charges = new ArrayList<>();
    private final List<String> invoked = new ArrayList<>();
    private boolean visible = true;

    @SuppressWarnings("unchecked")
    private static <T> T fake(Class<T> type, java.util.function.BiFunction<String, Object[], Object> handler) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, (proxy, method, args) -> {
            if (method.getName().equals("equals")) return proxy == args[0];
            if (method.getName().equals("hashCode")) return System.identityHashCode(proxy);
            return handler.apply(method.getName(), args == null ? new Object[0] : args);
        });
    }

    private static Callback callback(boolean direct, int limit) {
        return new Callback() {
            public String value() { return ""; }
            public boolean direct() { return direct; }
            public int limit() { return limit; }
            public String doc() { return ""; }
            public boolean getter() { return false; }
            public boolean setter() { return false; }
            public Class<? extends Annotation> annotationType() { return Callback.class; }
        };
    }

    private WackasmArchitecture architecture() {
        Component disk = fake(Component.class, (name, args) -> {
            switch (name) {
                case "canBeSeenFrom": return visible;
                case "annotation":
                    if (args[0].equals("missing")) return null;
                    return args[0].equals("read") ? callback(true, 15) : callback(false, Integer.MAX_VALUE);
                case "invoke": invoked.add((String) args[0]); return new Object[] {"ok"};
                default: return null;
            }
        });
        Network network = fake(Network.class, (name, args) -> "disk".equals(args.length > 0 ? args[0] : null) ? disk : null);
        Node self = fake(Node.class, (name, args) -> name.equals("network") ? network : null);
        Machine machine = fake(Machine.class, (name, args) -> {
            switch (name) {
                case "node": return self;
                case "consumeCallBudget": charges.add((Double) args[0]); return null;
                case "invoke": invoked.add("machine." + args[1]); return new Object[] {"via machine"};
                default: return null;
            }
        });
        return new WackasmArchitecture(machine);
    }

    @AfterEach
    void reset() {
        WackasmConfig.callBudgetMultiplier = 1.0;
    }

    @Test
    void multiplierOneChargesWhatOpenComputersDoes() throws Exception {
        // OC:M's Machine.invoke charges 1/limit for a direct call; so does ours.
        assertArrayEquals(new Object[] {"ok"}, architecture().call("disk", "read", new Object[0]));
        assertEquals(java.util.Arrays.asList("read"), invoked);
        assertEquals(1, charges.size());
        assertEquals(1.0 / 15, charges.get(0), 1e-12);
    }

    @Test
    void nullResultsBecomeAnEmptyArray() {
        // filesystem.read returns null at end of file in OpenComputers: Modernized.
        assertArrayEquals(new Object[0], WackasmArchitecture.results(null));
        Object[] some = {"x"};
        assertSame(some, WackasmArchitecture.results(some));
    }

    @Test
    void unknownMethodsAreNoSuchMethod() {
        // OC:M's own invoke would throw a NullPointerException here.
        assertThrows(NoSuchMethodException.class, () -> architecture().call("disk", "missing", new Object[0]));
        assertTrue(invoked.isEmpty());
    }

    @Test
    void directCallsCostLessWithAMultiplier() throws Exception {
        WackasmConfig.callBudgetMultiplier = 2.0;
        assertArrayEquals(new Object[] {"ok"}, architecture().call("disk", "read", new Object[0]));
        assertEquals(java.util.Arrays.asList("read"), invoked);
        assertEquals(1, charges.size());
        assertEquals(1.0 / 15 / 2.0, charges.get(0), 1e-12);
    }

    @Test
    void indirectCallsAreNotCharged() throws Exception {
        WackasmConfig.callBudgetMultiplier = 2.0;
        architecture().call("disk", "setLabel", new Object[0]);
        assertTrue(charges.isEmpty());
    }

    @Test
    void invisibleOrMissingComponentsAreRefused() {
        WackasmConfig.callBudgetMultiplier = 2.0;
        assertThrows(IllegalArgumentException.class, () -> architecture().call("nope", "read", new Object[0]));
        visible = false;
        assertThrows(IllegalArgumentException.class, () -> architecture().call("disk", "read", new Object[0]));
        assertTrue(invoked.isEmpty());
    }
}
