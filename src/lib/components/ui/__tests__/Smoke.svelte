<script lang="ts">
  /**
   * Test-only composition of the vendored components, for smoke.test.ts. Uses
   * the same `$lib/components/ui/<name>/index.js` paths the app lanes use, so
   * a broken barrel or alias fails here first.
   */
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Stepper } from "$lib/components/ui/stepper/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";

  let {
    onClick = () => {},
    switched = $bindable(false),
    dialogOpen = false,
  }: { onClick?: () => void; switched?: boolean; dialogOpen?: boolean } = $props();
</script>

<div>
  <Button onclick={onClick}>Retry</Button>
  <Badge variant="destructive">failed</Badge>
  <Label for="smoke-url">GitLab URL</Label>
  <Input id="smoke-url" value="https://gitlab.com" />
  <Switch aria-label="Notify" bind:checked={switched} />
  <Checkbox aria-label="Scheduled" checked />
  <Stepper
    steps={[
      { id: "account", label: "Account" },
      { id: "project", label: "Project" },
      { id: "review", label: "Review" },
    ]}
    current={1}
  />
  <Collapsible.Root open>
    <Collapsible.Trigger>deploy</Collapsible.Trigger>
    <Collapsible.Content>bridge jobs</Collapsible.Content>
  </Collapsible.Root>
  <Alert.Root variant="destructive">
    <Alert.Title>Token rejected</Alert.Title>
    <Alert.Description>401 from GitLab</Alert.Description>
  </Alert.Root>
</div>

<Dialog.Root open={dialogOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Write config.toml?</Dialog.Title>
      <Dialog.Description>Review before saving.</Dialog.Description>
    </Dialog.Header>
  </Dialog.Content>
</Dialog.Root>
