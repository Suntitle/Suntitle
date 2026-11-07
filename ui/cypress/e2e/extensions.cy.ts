import type { Extension, ExtensionSpec } from "../../src/stores/models"

const accessToken = "mock-token"

/** Build a resource whose API identity differs from its display name. */
function resource(spec?: Partial<ExtensionSpec>): Extension {
  return {
    name: "custom-captioner",
    displayName: "Custom captioner", sourceType: "local", source: "/opt/extensions/captioner", adapter: { nodeRole: "transcriber", execution: { command: "./captioner", runtime: "executable" } }, ...spec,
  }
}

/** Visit with authentication and a spec-conforming resource list. */
function visitAuthenticatedExtensions(items?: Extension[]) {
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/extensions", items ? { body: { items, resourceVersion: "1" } } : { fixture: "api/v1/extensions.json" }).as("getExtensions")
  cy.visit("/extensions", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", accessToken) } })
  cy.wait(["@getCurrentUser", "@getExtensions"])
}

/** Keep list responses consistent with successful registrations during polling. */
function stubRegistration(extension: Extension) {
  cy.fixture("api/v1/extensions.json").then((list) => {
    cy.intercept("GET", "**/api/v1/extensions", (request) => request.reply(list))
    cy.intercept("POST", "**/api/v1/extensions", (request) => {
      list.items.push(extension)
      request.reply({ statusCode: 201, body: extension })
    }).as("registerExtension")
  })
}

describe("Extensions page", () => {
  it("uses the existing node icon for each extension role", () => {
    const roles = [
      ["audio_source", "mic"],
      ["transcriber", "audio-waveform"],
      ["translator", "languages"],
      ["display", "monitor"],
      ["custom", "blocks"],
    ] as const
    visitAuthenticatedExtensions(roles.map(([nodeRole]) => ({
      ...resource({ displayName: `${nodeRole} extension`, adapter: { nodeRole, execution: { command: "./run" } } }),
      name: nodeRole,
    })))
    for (const [role, icon] of roles) {
      cy.contains("tr", `${role} extension`).find(`svg.lucide-${icon}`).should("be.visible")
    }
    cy.screenshot("extension-role-icons")
  })

  it("reads flat extensions and preserves resource status after an accepted refresh", () => {
    cy.fixture("api/v1/extensions.json").then((list) => {
      const refreshed = { ...list.items[0] }
      cy.intercept("POST", "**/api/v1/extensions/obs-captions/refresh", { statusCode: 202, body: refreshed }).as("refreshExtension")
    })
    visitAuthenticatedExtensions()
    cy.get("@getExtensions").its("response.body.items").then((items: Extension[]) => {
      for (const item of items) {
        expect(item).to.have.property("displayName")
        expect(item).to.have.property("name")
        for (const field of ["apiVersion", "kind", "metadata", "spec", "generation", "observedGeneration"]) expect(item).not.to.have.property(field)
      }
    })
    cy.contains("2 extensions").should("be.visible")
    cy.contains("tr", "obs-captions").within(() => {
      cy.contains("Published").should("be.visible")
      cy.get('button[aria-label="Actions for obs-captions"]').click()
    })
    cy.contains('[data-slot="dropdown-menu-item"]', "Refresh extension").click()
    cy.wait("@refreshExtension").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body ?? "").to.equal("")
    })
    cy.contains("tr", "obs-captions").should("contain.text", "Published")
    cy.contains("tr", "obs-captions").should("not.contain.text", "Previous artifact:")
  })

  for (const [sourceType, label, source] of [
    ["local", "Local", "/opt/extensions/captioner"],
    ["git", "Git", "https://github.com/example/captioner.git"],
    ["oci", "OCI", "registry.example.com/captioner:latest"],
    ["http", "HTTP", "https://example.com/hello.py"],
  ] as const) {
    it(`registers a ${label} source from its path or URL without manual manifest fields`, () => {
      visitAuthenticatedExtensions()
      stubRegistration(resource({ sourceType, source }))
      cy.contains("button", "Register extension").click()
      cy.get("#extension-name, #extension-command, #extension-bindings").should("not.exist")
      cy.get('input[type="file"]').should("not.exist")
      if (sourceType !== "local") {
        cy.get('button[aria-label="Source type"]').click()
        cy.contains('[data-slot="select-item"]', label).click()
      }
      cy.get("#extension-source").type(source)
      cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
      cy.wait("@registerExtension").then(({ request }) => {
        expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
        expect(request.body).to.deep.equal({ sourceType, source })
      })
      cy.get('[data-slot="dialog-content"]').should("not.exist")
      cy.contains("tr", "Custom captioner").should("contain.text", "Pending")
      cy.contains("3 extensions").should("be.visible")
    })
  }

  it("retains the source after manifest errors and clears it on cancel", () => {
    cy.intercept("POST", "**/api/v1/extensions", { statusCode: 422, body: { error: { message: "No manifest.yaml found in the extension directory." } } }).as("registerExtension")
    visitAuthenticatedExtensions()
    cy.contains("button", "Register extension").click()
    cy.contains('[data-slot="dialog-content"] button', "Register extension").should("be.disabled")
    cy.get("#extension-source").type("/opt/suntitle/extensions/hello")
    cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
    cy.wait("@registerExtension")
    cy.get('[role="alert"]').should("contain.text", "No manifest.yaml")
    cy.get("#extension-source").should("have.value", "/opt/suntitle/extensions/hello")
    cy.contains("button", "Cancel").click()
    cy.contains("button", "Register extension").click()
    cy.get("#extension-source").should("have.value", "")
    cy.get('[role="alert"]').should("not.exist")
  })

  it("disables registration while the source manifest is being read", () => {
    cy.intercept("POST", "**/api/v1/extensions", { statusCode: 201, body: resource(), delay: 1500 }).as("registerExtension")
    visitAuthenticatedExtensions()
    cy.contains("button", "Register extension").click()
    cy.get("#extension-source").type("/opt/suntitle/extensions/hello")
    cy.screenshot("extension-registration-source")
    cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
    cy.contains("button", "Registering…").should("be.disabled")
    cy.get("#extension-source").should("be.disabled")
    cy.wait("@registerExtension")
    cy.get('[data-slot="dialog-content"]').should("not.exist")
  })

  it("reconciles asynchronous publication and reports terminal failure", () => {
    cy.clock()
    visitAuthenticatedExtensions()
    cy.contains("2 extensions").should("be.visible")
    const extension = resource()
    extension.status = { observedGeneration: 1, conditions: [{ type: "Ready", status: "False", reason: "PreparationFailed", message: "Publication attempts exhausted", lastTransitionTime: "2026-11-06T12:00:00Z" }] }
    cy.intercept("GET", "**/api/v1/extensions", { body: { items: [extension], resourceVersion: "20" } }).as("reconcileExtensions")
    cy.tick(5000)
    cy.wait("@reconcileExtensions")
    cy.contains("tr", "Custom captioner").should("contain.text", "Failed").and("contain.text", "Publication attempts exhausted")
    cy.get('button[aria-label="Actions for custom-captioner"]').should("be.visible")
  })
})
