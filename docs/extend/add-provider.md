# Add A Provider

A provider is a model backend with its own context, streaming, tool-calling, and compatibility constraints. Keep provider integration focused on transport and model behavior rather than duplicating agent policy.

Provider-facing context should remain compact. Memory, skills, and activation traces should be summarized or progressively loaded instead of blindly appended to every request.

