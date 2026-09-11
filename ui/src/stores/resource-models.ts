import type { Host, HostResource, Node, NodeResource, NodeType } from "./models"

export function mapHost(resource: HostResource): Host {
  return {
    id: resource.metadata.uid,
    name: resource.spec.name,
    ip: resource.status?.ip ?? "Unknown",
    version: resource.status?.agentVersion ?? "Unknown",
    lastSeenAt: resource.status?.lastHeartbeat ?? "",
    metadata: resource.status?.reportedMetadata,
    status: resource.status?.presence === "Online" ? "online" : "offline",
  }
}

export function mapNode(resource: NodeResource, hosts: HostResource[]): Node {
  const host = hosts.find((host) => host.metadata.name === resource.spec.hostRef.name)
  const type = Object.keys(resource.spec.settings)[0] as NodeType
  return {
    id: resource.metadata.name,
    ...resource.spec,
    spec: resource.spec,
    type,
    settings: resource.spec.settings[type],
    inputNodeId: resource.spec.inputRef?.name ?? null,
    host: host ? mapHost(host) : {
      id: "", name: resource.spec.hostRef.name, ip: "Unknown", version: "Unknown",
      status: "offline", lastSeenAt: "",
    },
    status: resource.status?.phase === "Running" ? "connected"
      : resource.status?.phase === "Failed" ? "degraded" : "disconnected",
    createdAt: resource.metadata.creationTimestamp ?? "",
    updatedAt: "",
  } as Node
}
