const accessToken = "mock-token"

function visitAuthenticatedWorkspace() {
  cy.intercept("GET", "**/api/v1/auth/me", {
    fixture: "api/v1/auth/me.json",
  }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/hosts", {
    fixture: "api/v1/hosts.json",
  }).as("getHosts")
  cy.intercept("GET", "**/api/v1/nodes", {
    fixture: "api/v1/nodes.json",
  }).as("getNodes")

  cy.visit("/", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Nodes page", () => {
  it("uses the Nodes route and renders unrelated pipelines separately", () => {
    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])

    cy.contains("a", "Nodes").click()
    cy.location("pathname").should("equal", "/nodes")
    cy.wait("@getNodes")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)

    cy.get("h1").should("have.text", "Nodes").and("be.visible")
    cy.contains("7 nodes").should("be.visible")
    cy.get('[data-testid="topology-pipeline"]').should("have.length", 2)

    cy.get('[data-testid="topology-pipeline"]').eq(0).within(() => {
      cy.contains("Pipeline A").should("be.visible")
      cy.contains("Studio microphone").should("be.visible")
      cy.contains("Live transcriber").should("be.visible")
      cy.contains("English → Spanish").should("exist")
      cy.contains("Stage display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 3)
    })

    cy.get('[data-testid="topology-pipeline"]').eq(1).within(() => {
      cy.contains("Pipeline B").should("be.visible")
      cy.contains("Lobby microphone").should("be.visible")
      cy.contains("Lobby transcriber").should("be.visible")
      cy.contains("Lobby display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 2)
    })

    cy.reload()
    cy.wait(["@getCurrentUser", "@getNodes"])
    cy.location("pathname").should("equal", "/nodes")
    cy.get("h1").should("have.text", "Nodes")
  })

  it("renders desired connections before nodes report status", () => {
    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])
    cy.fixture("api/v1/nodes.json").then((body) => {
      body.items.forEach((node: { status?: unknown }) => { delete node.status })
      cy.intercept("GET", "**/api/v1/nodes", { body }).as("pendingNodes")
    })
    cy.contains("a", "Nodes").click()
    cy.wait("@pendingNodes")
    cy.get('[data-testid="topology-connection"]').should("have.length", 5)
    cy.contains('[data-testid="topology-node"]', "Live transcriber").within(() => {
      cy.contains("disconnected").should("be.visible")
    })
  })

  it("opens node settings, filters compatible inputs, and updates the node", () => {
    cy.intercept("GET", "**/api/v1/node-types", {
      fixture: "api/v1/node-types.json",
    }).as("getNodeTypes")
    cy.intercept("PUT", "**/api/v1/nodes/30000000-0000-4000-8000-000000000003", {
      fixture: "api/v1/nodes/30000000-0000-4000-8000-000000000003.json",
    }).as("updateNode")

    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])

    cy.contains("a", "Nodes").click()
    cy.wait("@getNodes")

    cy.contains('[data-testid="topology-node"]', "Live transcriber")
      .find('button[aria-label^="Open settings for Live transcriber"]')
      .click()
    cy.wait("@getNodeTypes")

    cy.get('[data-slot="dialog-title"]').should("have.text", "Update Live transcriber")
    cy.contains("section", "Node details").within(() => {
      cy.contains("Transcriber").should("be.visible")
      cy.contains("studio-host").should("be.visible")
      cy.contains("connected").should("be.visible")
    })
    cy.contains("section", "Current input").within(() => {
      cy.contains("Studio microphone").should("be.visible")
      cy.contains("Audio source · studio-host").should("be.visible")
    })

    cy.get('button[aria-label="Input"]').should("contain.text", "Studio microphone · studio-host").click()
    cy.get('[data-slot="select-content"]').within(() => {
      cy.contains('[data-slot="select-item"]', "Studio microphone · studio-host").should("be.visible")
      cy.contains('[data-slot="select-item"]', "Lobby microphone · studio-host").should("be.visible")
      cy.contains("Live transcriber").should("not.exist")
      cy.contains("English → Spanish").should("not.exist")
    })
    cy.get('[data-slot="select-item"]').contains("Studio microphone · studio-host").click()

    cy.contains("Accepts: Audio source").should("be.visible")
    cy.get("#node-name").clear().type("Updated transcriber")
    cy.get("#node-setting-model").clear().type("medium")
    cy.contains("button", "Save changes").click()

    cy.wait("@updateNode").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body).to.deep.equal({ spec: {
        name: "Updated transcriber",
        description: "English transcription",
        enabled: true,
        inputRef: { name: "30000000-0000-4000-8000-000000000001" },
        settings: { transcriber: {
          model: "medium",
          language: "en-US",
          task: "transcribe",
          beamSize: 5,
          vadEnabled: true,
        } },
        hostRef: { name: "studio-host-resource" },
        restartToken: "existing-restart",
      } })
    })

    cy.get('[data-slot="dialog-content"]').should("not.exist")
    cy.contains('[data-testid="topology-node"]', "Updated transcriber").should("be.visible")
  })

  it("restarts and disables a node from the actions dropdown after confirmation", () => {
    cy.intercept("POST", "**/api/v1/nodes/30000000-0000-4000-8000-000000000001/restart", {
      statusCode: 202,
      fixture: "api/v1/nodes/30000000-0000-4000-8000-000000000001/restart.json",
    }).as("restartNode")
    cy.intercept("PUT", "**/api/v1/nodes/30000000-0000-4000-8000-000000000001", {
      fixture: "fixtures/nodes/studio-microphone-disabled.json",
    }).as("disableNode")

    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])
    cy.contains("a", "Nodes").click()
    cy.wait("@getNodes")

    cy.contains('[data-testid="topology-node"]', "Studio microphone")
      .find('button[aria-label="Actions for Studio microphone"]')
      .click()
    cy.contains('[data-slot="dropdown-menu-item"]', "Restart node").click()

    cy.get('[data-slot="alert-dialog-title"]').should("have.text", "Restart Studio microphone?")
    cy.contains('[data-slot="alert-dialog-content"]', "may pause briefly").should("be.visible")

    cy.window().then((window) => {
      let renderedDisableConfirmation = false
      const observer = new window.MutationObserver(() => {
        renderedDisableConfirmation ||= window.document.body.textContent?.includes("Disable Studio microphone?") ?? false
      })
      observer.observe(window.document.body, { childList: true, subtree: true, characterData: true })

      cy.contains('button[data-slot="alert-dialog-cancel"]', "Cancel").click()
      cy.wait(500).then(() => {
        observer.disconnect()
        expect(renderedDisableConfirmation).to.equal(false)
      })
    })
    cy.get('[data-slot="alert-dialog-content"]').should("not.exist")

    cy.contains('[data-testid="topology-node"]', "Studio microphone")
      .find('button[aria-label="Actions for Studio microphone"]')
      .click()
    cy.contains('[data-slot="dropdown-menu-item"]', "Restart node").click()
    cy.contains('button[data-slot="alert-dialog-action"]', "Restart node").click()

    cy.wait("@restartNode").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
    })
    cy.get('[data-slot="alert-dialog-content"]').should("not.exist")

    cy.contains('[data-testid="topology-node"]', "Studio microphone")
      .find('button[aria-label="Actions for Studio microphone"]')
      .click()
    cy.contains('[data-slot="dropdown-menu-item"]', "Disable node").click()

    cy.get('[data-slot="alert-dialog-title"]').should("have.text", "Disable Studio microphone?")
    cy.contains('[data-slot="alert-dialog-content"]', "may interrupt downstream output").should("be.visible")
    cy.contains('button[data-slot="alert-dialog-action"]', "Disable node").click()

    cy.wait("@disableNode").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body).to.deep.equal({ spec: {
        name: "Studio microphone",
        description: "Primary audio input",
        enabled: false,
        inputRef: null,
        settings: { audio_source: {
          sampleRateHz: 48000,
          channels: 1,
          encoding: "pcm_s16le",
          deviceName: "USB Audio Device",
        } },
        hostRef: { name: "studio-host-resource" },
        restartToken: "existing-restart",
      } })
    })

    cy.contains('[data-testid="topology-node"]', "Studio microphone").within(() => {
      cy.contains("disabled").should("be.visible")
    })
  })
})
