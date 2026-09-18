<script lang="ts">
  /**
   * Test-only composition of the vendored components, for smoke.test.ts. Uses
   * the same `$lib/components/ui/<name>/index.js` paths the app lanes use, so
   * a broken barrel or alias fails here first.
   */
  import * as Accordion from "$lib/components/ui/accordion/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Progress } from "$lib/components/ui/progress/index.js";
  import { ScrollArea } from "$lib/components/ui/scroll-area/index.js";
  import { Separator } from "$lib/components/ui/separator/index.js";
  import { Stepper } from "$lib/components/ui/stepper/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import * as Tabs from "$lib/components/ui/tabs/index.js";

  let {
    onClick = () => {},
    switched = $bindable(false),
    dialogOpen = false,
  }: { onClick?: () => void; switched?: boolean; dialogOpen?: boolean } = $props();
</script>

<Card.Root>
  <Card.Header>
    <Card.Title>Pipeline 42</Card.Title>
    <Card.Description>main</Card.Description>
  </Card.Header>
  <Card.Content>
    <Button onclick={onClick}>Retry</Button>
    <Badge variant="destructive">failed</Badge>
    <Separator />
    <Label for="smoke-url">GitLab URL</Label>
    <Input id="smoke-url" value="https://gitlab.com" />
    <Switch aria-label="Notify" bind:checked={switched} />
    <Checkbox aria-label="Scheduled" checked />
    <Progress value={40} max={100} />
    <Stepper
      steps={[
        { id: "account", label: "Account" },
        { id: "project", label: "Project" },
        { id: "review", label: "Review" },
      ]}
      current={1}
    />
    <Tabs.Root value="all">
      <Tabs.List>
        <Tabs.Trigger value="all">All</Tabs.Trigger>
        <Tabs.Trigger value="failures">Failures</Tabs.Trigger>
      </Tabs.List>
      <Tabs.Content value="all">every job</Tabs.Content>
      <Tabs.Content value="failures">failures only</Tabs.Content>
    </Tabs.Root>
    <Collapsible.Root open>
      <Collapsible.Trigger>deploy</Collapsible.Trigger>
      <Collapsible.Content>bridge jobs</Collapsible.Content>
    </Collapsible.Root>
    <Accordion.Root type="single" value="a">
      <Accordion.Item value="a">
        <Accordion.Trigger>Stage build</Accordion.Trigger>
        <Accordion.Content>compile</Accordion.Content>
      </Accordion.Item>
    </Accordion.Root>
    <ScrollArea class="h-10">long list</ScrollArea>
    <Alert.Root variant="destructive">
      <Alert.Title>Token rejected</Alert.Title>
      <Alert.Description>401 from GitLab</Alert.Description>
    </Alert.Root>
  </Card.Content>
</Card.Root>

<Dialog.Root open={dialogOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Write config.toml?</Dialog.Title>
      <Dialog.Description>Review before saving.</Dialog.Description>
    </Dialog.Header>
  </Dialog.Content>
</Dialog.Root>
