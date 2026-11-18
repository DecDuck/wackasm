package dev.wackasm.oc;

import java.io.IOException;
import java.io.InputStream;

import li.cil.oc.api.FileSystem;
import li.cil.oc.api.Items;
import net.minecraft.item.EnumDyeColor;
import net.minecraft.item.ItemStack;
import net.minecraft.item.crafting.Ingredient;
import net.minecraft.util.ResourceLocation;
import net.minecraftforge.fml.common.Mod;
import net.minecraftforge.fml.common.event.FMLInitializationEvent;
import net.minecraftforge.fml.common.event.FMLPreInitializationEvent;
import net.minecraftforge.fml.common.event.FMLServerStartingEvent;
import net.minecraftforge.fml.common.registry.GameRegistry;
import org.apache.logging.log4j.Logger;

/**
 * Registers the WebAssembly architecture with OpenComputers, along with the
 * two items it boots from. Together they mirror Lua's Lua BIOS EEPROM and
 * OpenOS floppy:
 * <ul>
 *   <li><b>EEPROM (WebAssembly BIOS)</b>: boots {@code /init.wasm} from a
 *       filesystem (see {@code crates/wackasm-oc-bios}).</li>
 *   <li><b>wackasm shell</b> floppy: a small shell installed as {@code /init.wasm}
 *       (see {@code crates/wackasm-oc-guest}).</li>
 * </ul>
 *
 * <p>The native library is only loaded on the logical server, so clients
 * without a build for their platform can still join.
 */
@Mod(
        modid = WackasmMod.MOD_ID,
        name = "Wackasm for OpenComputers",
        useMetadata = true,
        dependencies = "required-after:opencomputers@[1.7.5,)",
        acceptableRemoteVersions = "*")
public final class WackasmMod {
    public static final String MOD_ID = "wackasm";
    /** OpenComputers' default {@code eepromSize}. */
    private static final int EEPROM_SIZE = 4096;

    static Logger log;

    @Mod.EventHandler
    public void preInit(FMLPreInitializationEvent event) {
        log = event.getModLog();
        li.cil.oc.api.Machine.add(WackasmArchitecture.class);
        log.info("Registered the WebAssembly (wackasm) architecture with OpenComputers");
    }

    @Mod.EventHandler
    public void init(FMLInitializationEvent event) {
        byte[] bios = resource("/assets/wackasm/bios.wasm");
        if (bios.length > EEPROM_SIZE) {
            log.warn("The wasm BIOS is {} bytes, more than a default EEPROM holds ({})", bios.length, EEPROM_SIZE);
        }
        // Writable like the Lua BIOS: the BIOS stores its boot address in the
        // EEPROM's data, and players can reflash it.
        ItemStack biosEeprom = Items.registerEEPROM("EEPROM (WebAssembly BIOS)", bios, null, false);
        ItemStack shellFloppy = Items.registerFloppy("wackasm shell", EnumDyeColor.LIME,
                () -> FileSystem.fromClass(WackasmMod.class, MOD_ID, "loot/shell"), true);

        // OC only puts registered EEPROMs and floppies in the creative tab, so
        // give them survival recipes. OC uses <item> + manual for the Lua BIOS
        // and OpenOS, so these use paper to stay clear of those.
        Ingredient paper = Ingredient.fromItem(net.minecraft.init.Items.PAPER);
        GameRegistry.addShapelessRecipe(new ResourceLocation(MOD_ID, "wasm_bios"), null, biosEeprom,
                Ingredient.fromStacks(Items.get("eeprom").createItemStack(1)), paper);
        GameRegistry.addShapelessRecipe(new ResourceLocation(MOD_ID, "shell_floppy"), null, shellFloppy,
                Ingredient.fromStacks(Items.get("floppy").createItemStack(1)), paper);
    }

    /** Loads the native library up front so a missing build fails here, not on the first boot. */
    @Mod.EventHandler
    public void serverStarting(FMLServerStartingEvent event) {
        try {
            String workDir = WackasmPaths.workDir();
            WackasmNative.destroy(WackasmNative.create(workDir, "selftest", "selftest"));
            log.info("Native library loaded for {}; work dir is {}", NativeLoader.platform(), workDir);
        } catch (Throwable t) {
            log.error("wackasm native library is unavailable on {}; WebAssembly computers will not boot",
                    NativeLoader.platform(), t);
        }
    }

    private static byte[] resource(String path) {
        try (InputStream in = WackasmMod.class.getResourceAsStream(path)) {
            if (in == null) throw new IllegalStateException("missing " + path + " in the mod jar");
            return NativeLoader.readAll(in);
        } catch (IOException e) {
            throw new IllegalStateException("reading " + path, e);
        }
    }
}
