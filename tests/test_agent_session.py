import unittest

from src.open_llm_vtuber.agent.agents.basic_memory_agent import BasicMemoryAgent
from src.open_llm_vtuber.agent.stateless_llm.stateless_llm_interface import (
    StatelessLLMInterface,
)


class FakeStatelessLLM(StatelessLLMInterface):
    async def chat_completion(self, messages, system=None, tools=None):
        if False:
            yield ""


class BasicMemoryAgentSessionTests(unittest.TestCase):
    def test_sessions_share_llm_but_isolate_memory(self):
        llm = FakeStatelessLLM()
        template = BasicMemoryAgent(
            llm=llm,
            system="Test system",
            live2d_model=None,
            segment_method="regex",
        )

        first = template.create_session()
        second = template.create_session()
        first._add_message("private message", "user")

        self.assertIs(first._llm, llm)
        self.assertIs(second._llm, llm)
        self.assertIsNot(first, second)
        self.assertEqual(first._memory, [{"role": "user", "content": "private message"}])
        self.assertEqual(second._memory, [])
        self.assertEqual(template._memory, [])
        self.assertEqual(first._system, second._system)


if __name__ == "__main__":
    unittest.main()
