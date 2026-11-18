// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightThemeNova from 'starlight-theme-nova';

// Published to GitHub Pages at https://decduck.github.io/wackasm/.
// SITE and BASE can be overridden, for example for a fork or a custom domain.
const site = process.env.SITE ?? 'https://decduck.github.io';
const base = process.env.BASE ?? '/wackasm';

export default defineConfig({
  site,
  base,
  integrations: [
    starlight({
      title: 'wackasm',
      // Shown next to the title (the same icon as the favicon).
      logo: { src: './src/assets/logo.svg', alt: '' },
      plugins: [starlightThemeNova()],
      description: 'WebAssembly computers for OpenComputers, running on the wackasm interpreter.',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/DecDuck/wackasm' }],
      editLink: { baseUrl: 'https://github.com/DecDuck/wackasm/edit/master/docs/' },
      lastUpdated: true,
      sidebar: [
        { label: 'Start here', items: [{ autogenerate: { directory: 'start' } }] },
        { label: 'Using a computer', items: [{ autogenerate: { directory: 'using' } }] },
        { label: 'Writing programs', items: [{ autogenerate: { directory: 'programs' } }] },
        {
          label: 'Reference',
          items: [
            { autogenerate: { directory: 'reference' } },
            // Built by CI from the Rust sources (see contributing/building).
            { label: 'Rust API (rustdoc)', link: '/rustdoc/wackasm_opencomputers/' },
          ],
        },
        { label: 'Contributing', items: [{ autogenerate: { directory: 'contributing' } }] },
      ],
    }),
  ],
});
