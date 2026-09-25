-- Where each agent's "new items" trigger has read up to: the row of the
-- last item handed to it, which grows with every item stored. Set at install, so an agent starts
-- with what arrives from then on and asks for history itself if it wants.
ALTER TABLE agent ADD COLUMN cursor TEXT;
