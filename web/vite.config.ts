import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			// A single-page app: the hub serves 200.html for every route.
			adapter: adapter({ fallback: '200.html', precompress: false })
		})
	],
	server: {
		// `npm run dev` talks to a running `watcher run`.
		proxy: { '/api': 'http://127.0.0.1:8080' }
	}
});
