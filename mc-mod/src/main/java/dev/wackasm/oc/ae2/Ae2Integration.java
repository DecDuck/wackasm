package dev.wackasm.oc.ae2;

import dev.wackasm.oc.WackasmMod;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.SoundType;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.capabilities.RegisterCapabilitiesEvent;
import net.neoforged.neoforge.event.BuildCreativeModeTabContentsEvent;
import net.neoforged.neoforge.registries.DeferredBlock;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredItem;
import net.neoforged.neoforge.registries.DeferredRegister;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Applied Energistics 2 integration: the computer-controlled pattern provider.
 * Only loaded when AE2 is installed (see {@link WackasmMod}); nothing outside
 * this package refers to AE2.
 */
public final class Ae2Integration {
    public static final String MOD_ID = "ae2";

    static final Logger log = LoggerFactory.getLogger(WackasmMod.MOD_ID + "/ae2");

    private static final DeferredRegister.Blocks BLOCKS = DeferredRegister.createBlocks(WackasmMod.MOD_ID);
    private static final DeferredRegister.Items ITEMS = DeferredRegister.createItems(WackasmMod.MOD_ID);
    private static final DeferredRegister<BlockEntityType<?>> BLOCK_ENTITIES =
            DeferredRegister.create(Registries.BLOCK_ENTITY_TYPE, WackasmMod.MOD_ID);

    static final DeferredBlock<PatternProviderBlock> PATTERN_PROVIDER = BLOCKS.register("pattern_provider",
            () -> new PatternProviderBlock(BlockBehaviour.Properties.of()
                    .strength(2.2f, 11f)
                    .sound(SoundType.METAL)));

    static final DeferredItem<BlockItem> PATTERN_PROVIDER_ITEM =
            ITEMS.registerSimpleBlockItem("pattern_provider", PATTERN_PROVIDER, new Item.Properties());

    @SuppressWarnings("DataFlowIssue") // the data fixer type may be null
    static final DeferredHolder<BlockEntityType<?>, BlockEntityType<PatternProviderBlockEntity>>
            PATTERN_PROVIDER_BLOCK_ENTITY = BLOCK_ENTITIES.register("pattern_provider",
                    () -> BlockEntityType.Builder.of(PatternProviderBlockEntity::new, PATTERN_PROVIDER.get()).build(null));

    private Ae2Integration() {}

    public static void register(IEventBus modBus) {
        BLOCKS.register(modBus);
        ITEMS.register(modBus);
        BLOCK_ENTITIES.register(modBus);
        modBus.addListener(RegisterCapabilitiesEvent.class, event -> event.registerBlockEntity(
                Capabilities.ItemHandler.BLOCK, PATTERN_PROVIDER_BLOCK_ENTITY.get(),
                (be, side) -> be.returnInventory()));
        // Next to OpenComputers' own blocks; it has one tab.
        modBus.addListener(BuildCreativeModeTabContentsEvent.class, event -> {
            if (event.getTabKey().location().getNamespace().equals("opencomputers")) {
                event.accept(PATTERN_PROVIDER_ITEM);
            }
        });
        log.info("Applied Energistics 2 found; adding the computer pattern provider");
    }
}
