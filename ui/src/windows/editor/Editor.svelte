<script lang="ts">
  import { SvelteFlowProvider } from "@xyflow/svelte";
  import Workbench from "./Workbench.svelte";
</script>

<!-- The provider lets the workbench use xyflow's hooks outside the canvas. -->
<SvelteFlowProvider>
  <Workbench />
</SvelteFlowProvider>
