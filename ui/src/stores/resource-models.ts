import type { Extension, Host, HostResource, Node, NodeResource, NodeRole } from "./models"

export function mapHost(resource: HostResource): Host {
  return {
    id: resource.name,
    name: resource.displayName,
    ip: resource.status?.ip ?? "Unknown",
    version: resource.status?.agentVersion ?? "Unknown",
    lastSeenAt: resource.status?.lastHeartbeat ?? "",
    metadata: resource.status?.reportedMetadata,
    status: resource.status?.presence === "Online" ? "online" : "offline",
  }
}

/** Resolve custom icons while keeping execution and built-in types independent. */
export function mapNode(resource: NodeResource, hosts: HostResource[], extensions: Extension[] = []): Node {
  const { name: resourceName, status, ...spec } = resource
  const host = hosts.find((host) => host.name === resource.hostRef.name)
  const extension = extensions.find((item) => item.name === resource.extensionRef?.name)
  const type = resource.extensionRef
    ? "custom"
    : Object.keys(resource.settings)[0] as NodeRole
  return {
    id: resourceName,
    extensionName: extension?.displayName ?? resource.extensionRef?.name,
    ...spec,
    name: resource.displayName,
    spec,
    type,
    ...(type === "custom" ? {
      inputType: extension?.adapter.inputType ?? status?.resolvedAdapter?.inputType,
      outputType: extension?.adapter.outputType ?? status?.resolvedAdapter?.outputType,
    } : {}),
    settings: resource.settings.custom ?? resource.settings[type],
    inputNodeId: resource.inputRef?.name ?? null,
    host: host ? mapHost(host) : {
      id: "", name: resource.hostRef.name, ip: "Unknown", version: "Unknown",
      status: "offline", lastSeenAt: "",
    },
    status: status?.phase === "Running" ? "connected"
      : status?.phase === "Failed" ? "degraded" : "disconnected",
    createdAt: "",
    updatedAt: "",
  } as Node
}
