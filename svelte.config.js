// SPDX-FileCopyrightText: Distronode Corporation
//
// SPDX-License-Identifier: Apache-2.0

import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// `vitePreprocess` is what makes `<script lang="ts">` work in a .svelte file,
// for both the build and `svelte-check`.
export default { preprocess: vitePreprocess() };
