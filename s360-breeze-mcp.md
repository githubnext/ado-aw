Authentication and Access
The S360 MCP Server uses Microsoft authentication and respects existing S360 permissions:

Authentication: Microsoft Azure AD (Entra) authentication required.
Authorization: Users can only access data they have permissions to view in S360
Service Scope: Access is limited to services and KPIs the user can view through the S360 web interface
Programmatic Access: Build Your Own MCP Client
This section helps developers integrate the S360 MCP Server into their own applications, agents, and agentic workflows.

Endpoints
Environment	Base url
TEST	https://mcp.vnext.s360test.msftcloudes.com/
PROD	https://mcp.vnext.s360.msftcloudes.com/
Authentication
The MCP server supports Azure Active Directory (Azure AD) Bearer token authentication via:

User delegated (On-Behalf-Of) flow (your client acts on behalf of a signed-in user)
Service-to-service flow
Use User Auth when you must execute MCP tools explicitly on behalf of an individual user and preserve user context/auditing. Use the service to service flow when you are running backend / automation scenarios without per-user context.

You can find more information about the MCP server auth scheme here.

Supported Issuing Tenants
S360 MCP currently only accepts tokens issued by the following Microsoft tenants:

Supported Tenant	Applies To (User / MSI)
CORP	User & MSI
Tokens issued from other tenants will be rejected.

S360 MCP Auth Resource / Scope Endpoints
Environment	User Auth Scope (Delegated / OBO)	MSI Auth Scope (Resource / Scope Base)
TEST	api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb/mcp-user	api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb
PROD	api://08654c87-a8c1-4098-a44b-079efd603fdc/mcp-user	api://08654c87-a8c1-4098-a44b-079efd603fdc
1. User Authentication (Delegated / On-Behalf-Of)
When an MCP client (web app, service or agent framework) needs to call the S360 MCP Server on behalf of a user, it must perform the Azure AD OAuth 2.0 On-Behalf-Of (OBO) flow to exchange the user token it received for an MCP-scoped token.

Additional Onboarding Steps (one-time)
Provision (or identify) an Azure AD application (App Registration) in the AME tenant for your client front-end / API.
Assign MCP delegated permissions to that application by following these steps:
On your app registration in the Azure portal, select Add a permission.
In the Request API permissions flyout, select the APIs my organization uses tab.
In the search input, type S360 Breeze Mcp Prod or S360 Breeze Mcp Test to find the MCP app, then select it from the list.
Select Delegated permissions.
From the list of available delegated permissions, select the checkbox for mcp-user.
Select Add permissions to save the permissions to your app registration.
Ensure your client app is multi-tenant (only AME and Corp) and allows users from other tenants. Ensure a Service Principal is created in the Corp tenant.
Email breezehelp@microsoft.com requesting authorization for your client app to use S360 MCP OBO (delegated) access. Include ALL details listed below.
Information to include in the authorization request email
Provide these fields plainly in the email body:

App Id (AAD application / client id)
App Tenant Id (GUID of tenant owning the app. This should be AME)
Environment(s) requested (TEST, PROD or both)
Requesting Team name
Requesting Team Service Tree Id (GUID or path)
Justification (scenarios, why delegated access is needed)
OBO Flow Summary
User signs in to your client front end and obtains an access token where aud = your client API/App (App1).
Your client backend receives the user token.
Backend calls Azure AD token endpoint performing OBO, presenting the user token as the user assertion and requesting scope api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb/mcp-user (or .default).
Azure AD returns an MCP token (aud = 6833b4aa-2e50-42b8-b3d9-2b0114fc39cb).
Backend uses this MCP token to connect to MCP and invoke tools on behalf of the user.
Local Testing (Interactive User Token)
For local prototyping you can still obtain a user token directly (TEST example) and call MCP (not OBO) when explicitly allowed:

# Run in PowerShell (Admin if needed)
# Install-Module -Name MSAL.PS -Force
Import-Module MSAL.PS
$token = Get-MsalToken -ClientId "6833b4aa-2e50-42b8-b3d9-2b0114fc39cb" `
                      -TenantId "72f988bf-86f1-41af-91ab-2d7cd011db47" `
                      -Scopes "api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb/mcp-user" `
                      -Interactive
Write-Host "Access Token: $($token.AccessToken)"
$token.AccessToken | clip
For production scenarios use the full OBO flow; do not pass raw user tokens from the browser directly to MCP.

2. Service-to-Service Authentication
Service-to-service authentication is supported for approved service principals. Approved service principals can be granted scoped, read-only access per environment. Write operations (such as setting ETAs or action item owners) require user context and are not available via service tokens.

To request service-to-service access, email breezehelp@microsoft.com with your app registration details, the environment(s) requested, and a justification for why user context is not viable (e.g., scheduled jobs, notifications, or reporting).

Code Examples
C#
using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;
using Microsoft.Identity.Client;
using ModelContextProtocol.Client;
using ModelContextProtocol.Protocol;

var builder = Host.CreateApplicationBuilder();

builder.Configuration
    .AddEnvironmentVariables()
    .AddUserSecrets<Program>();


using var loggerFactory = LoggerFactory.Create(builder => { });

ILogger logger = loggerFactory.CreateLogger<Program>();

// Connect to an MCP server
Console.WriteLine("Connecting client to MCP server");

// This must be a user JWT token with scope api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb/mcp-user
var token = await GetUserTokenAsync("api://6833b4aa-2e50-42b8-b3d9-2b0114fc39cb/mcp-user", "<your client id. You can use 04b07795-8ddb-461a-bbee-02f9e1bf7b46 (az cli) for testing>");

var mcpClient = await McpClient.CreateAsync(
    new HttpClientTransport(new HttpClientTransportOptions
    {
        Endpoint = new Uri("https://mcp.vnext.s360test.msftcloudes.com/"),
        ConnectionTimeout = new TimeSpan(0, 0, 100),
        Name = "StreamableHttp MCP Client",
        TransportMode = HttpTransportMode.StreamableHttp,
        AdditionalHeaders = new Dictionary<string, string>
        {
            { "Authorization", $"Bearer {token}" }
        },
    }, loggerFactory),
    loggerFactory: loggerFactory
    );

// Get all available tools
Console.WriteLine("Available Tools list from S360 MCP Server:");

IList<McpClientTool> tools = new List<McpClientTool>();

try
{
    tools = await mcpClient.ListToolsAsync();
    int count = 1;
    foreach (var tool in tools)
    {
        Console.WriteLine($"{count}.{tool.Name}");
        Console.WriteLine($"Description: {tool.Description}");
        count++;
    }
}
catch (Exception ex)
{
    logger.LogError(ex, "An error occurred while listing tools.");
}

Console.WriteLine("Call tool to get kpi info");

CallToolResult toolResult = await mcpClient.CallToolAsync(
    "search_s360_kpi_metadata",
    new Dictionary<string, object?>() { { "request", new Dictionary<string, object?> { { "kpiNameSearchTerm", "security" } } } },
    cancellationToken: CancellationToken.None);

var content = toolResult.Content.First(c => c.Type == "text") as TextContentBlock;
Console.WriteLine(content?.Text);

static async Task<string> GetUserTokenAsync(string scope, string clientId)
{
    var publicClient = PublicClientApplicationBuilder
        .Create(clientId)
        .WithAuthority("https://login.microsoftonline.com/72f988bf-86f1-41af-91ab-2d7cd011db47") // Microsoft Tenant ID
        .WithRedirectUri("http://localhost")
        .Build();

    try
    {
        result = await publicClient
            .AcquireTokenInteractive([scope])
            .WithPrompt(Microsoft.Identity.Client.Prompt.SelectAccount)
            .ExecuteAsync()
            .ConfigureAwait(false);

        return result.AccessToken;
    }
    catch (Exception ex)
    {
        Console.WriteLine($"Error acquiring user token for scope {scope}: {ex.Message}");
        throw;
    }
}