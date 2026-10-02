/** Mount point for the shell: `mount(Wizard, { target, props: { api, initial, onFinish, onSkip } })`. */
export { default as Wizard } from "./Wizard.svelte";
export type * from "./api";
export { needsConfirm, issuesOf } from "./api";
export { messageOf } from "../../lib/format";
